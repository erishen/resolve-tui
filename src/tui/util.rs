//! 工具函数：路径显示、剪贴板、审批参数美化。

/// 按字符数截断并加省略号；绝不按字节切（否则中文会切进 UTF-8 序列中间导致 panic）。
pub(crate) fn truncate_ellipsis(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let head: String = s.chars().take(max_chars).collect();
    format!("{head}…")
}

/// 把路径解析为可读形式：能 canonicalize 时给绝对路径，否则原样返回。
pub(crate) fn display_path(path: &str) -> String {
    std::path::Path::new(path)
        .canonicalize()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| path.to_string())
}

/// 通过 OSC 52 转义序列把文本写入系统剪贴板（大多数终端模拟器支持）。
/// 直接写 stdout：终端解析该控制序列后接管剪贴板，不影响屏幕内容。
pub(crate) fn copy_to_clipboard(text: &str) -> std::io::Result<()> {
    use base64::Engine as _;
    use std::io::Write as _;
    let encoded = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
    let seq = format!("\x1b]52;c;{encoded}\x07");
    let mut out = std::io::stdout();
    out.write_all(seq.as_bytes())?;
    out.flush()
}

/// 把工具调用的 arguments JSON 渲染成一行可读命令（审批提示用）。
/// 优先提取 `command` / `path` 字段；解析失败或其它结构则压缩成单行。
pub(crate) fn pretty_args(name: &str, args: &str) -> String {
    const MAX: usize = 200;
    let render = |s: String| {
        if s.chars().count() > MAX {
            let head: String = s.chars().take(MAX).collect();
            format!("{head}…")
        } else {
            s
        }
    };
    match serde_json::from_str::<serde_json::Value>(args) {
        Ok(v) => {
            for key in ["command", "path"] {
                if let Some(s) = v.get(key).and_then(|x| x.as_str()) {
                    return render(format!("{name} {s}"));
                }
            }
            render(format!("{name} {v}"))
        }
        Err(_) => render(format!("{name} {args}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 审批提示：优先提取 command/path 字段渲染成单行命令。
    #[test]
    fn approval_args_render_single_line() {
        assert_eq!(
            super::pretty_args("shell", r#"{"command":"ls -la"}"#),
            "shell ls -la"
        );
        assert_eq!(
            super::pretty_args("read_file", r#"{"path":"/tmp/a.txt"}"#),
            "read_file /tmp/a.txt"
        );
        assert_eq!(super::pretty_args("weird", "not-json"), "weird not-json");
    }

    // 回归：截断必须按字符进行——此前按字节切中文会落在 UTF-8 序列中间直接 panic
    // （"end byte index 400 is not a char boundary; it is inside '在'"）。
    #[test]
    fn truncate_ellipsis_never_splits_utf8_char() {
        // 399 个 ASCII 后跟「在」（3 字节）：字节下标 400 恰好在它中间。
        let s = format!("{}{}", "a".repeat(399), "在".repeat(50));
        let out = truncate_ellipsis(&s, 400);
        assert_eq!(out.chars().count(), 401, "400 字符 + 省略号");
        assert!(out.ends_with('…'));

        // 纯中文同样安全；未超长时原样返回、不加省略号。
        let cn = "你好世界".repeat(10);
        assert_eq!(truncate_ellipsis(&cn, 100), cn);
        let short = "短文本";
        assert_eq!(truncate_ellipsis(short, 400), short);
    }
}
