//! 从模型回复中抽取 rhai `detect` 检测器源码。

/// 从模型回复中抽取 rhai `detect` 函数；模型判定不可解（NONE）时返回 `None`。
pub fn extract_code(output: &str) -> Option<String> {
    let t = output.trim();
    if t.is_empty() {
        return None;
    }
    let lowered = t.to_lowercase();
    if lowered == "none"
        || ["无", "无法", "不能", "不需要"].contains(&t)
        || (lowered.starts_with("none") && !t.contains("fn detect"))
    {
        return None;
    }
    if let Some(inner) = extract_fenced(t) {
        return Some(inner);
    }
    if t.contains("fn detect") {
        return extract_fn_block(t);
    }
    None
}

/// 抽取 ```lang ... ``` 围栏块内容；首行若是语言标识则跳过。
fn extract_fenced(s: &str) -> Option<String> {
    let parts: Vec<&str> = s.split("```").collect();
    if parts.len() < 3 {
        return None;
    }
    let inner = parts[1];
    let mut lines = inner.lines().peekable();
    // 首行若只是语言名（无空格/无括号），视为标识行跳过。
    if let Some(first) = lines.peek()
        && !first.contains(' ')
        && !first.contains('{')
        && !first.is_empty()
    {
        let _ = lines.next();
    }
    let code: String = lines.collect::<Vec<_>>().join("\n");
    let code = code.trim().to_string();
    if code.contains("fn detect") {
        Some(code)
    } else {
        None
    }
}

/// 从 `fn detect` 起，按花括号配平截取完整函数体（忽略字符串/字符字面量内的括号）。
fn extract_fn_block(src: &str) -> Option<String> {
    let start = src.find("fn detect")?;
    let body = &src[start..];
    let mut depth = 0i32;
    let mut started = false;
    let mut in_str = false;
    let mut in_ch = false;
    let mut esc = false;
    let mut buf = String::new();
    for c in body.chars() {
        buf.push(c);
        if esc {
            esc = false;
            continue;
        }
        match c {
            '\\' if in_str || in_ch => esc = true,
            '"' => in_str = !in_str,
            '\'' => in_ch = !in_ch,
            '{' if !in_str && !in_ch => {
                depth += 1;
                started = true;
            }
            '}' if !in_str && !in_ch && started => {
                depth -= 1;
                if depth == 0 {
                    return Some(buf.trim().to_string());
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_declines_on_none() {
        assert!(extract_code("NONE").is_none());
        assert!(extract_code("无法确定性解决").is_none());
    }

    #[test]
    fn extract_pulls_fenced_rhai() {
        let out = "好的：\n```rhai\nfn detect(text) { \"x\" }\n```\n收尾";
        let code = extract_code(out).expect("应抽出函数");
        assert!(code.contains("fn detect"));
    }

    #[test]
    fn extract_pulls_bare_fn() {
        let code = extract_code("fn detect(text) { text }").unwrap();
        assert!(code.contains("fn detect"));
    }
}
