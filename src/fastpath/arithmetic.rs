//! 安全算术：手写递归下降解析器，绝不调用任何 eval/外部解释器。
use super::*;

// -- 安全算术 -------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
enum Tk {
    Num(f64),
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Pow,
    LParen,
    RParen,
}

fn tokenize(expr: &str) -> Result<Vec<Tk>, ()> {
    let chars: Vec<char> = expr.chars().collect();
    let mut i = 0;
    let mut out = Vec::new();
    while i < chars.len() {
        let c = chars[i];
        match c {
            '0'..='9' | '.' => {
                let mut s = String::new();
                let mut dot = false;
                while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                    if chars[i] == '.' {
                        if dot {
                            return Err(());
                        }
                        dot = true;
                    }
                    s.push(chars[i]);
                    i += 1;
                }
                let n: f64 = s.parse().map_err(|_| ())?;
                out.push(Tk::Num(n));
            }
            '+' => {
                out.push(Tk::Plus);
                i += 1;
            }
            '-' => {
                out.push(Tk::Minus);
                i += 1;
            }
            '*' => {
                if i + 1 < chars.len() && chars[i + 1] == '*' {
                    out.push(Tk::Pow);
                    i += 2;
                } else {
                    out.push(Tk::Star);
                    i += 1;
                }
            }
            '/' => {
                out.push(Tk::Slash);
                i += 1;
            }
            '%' => {
                out.push(Tk::Percent);
                i += 1;
            }
            '(' => {
                out.push(Tk::LParen);
                i += 1;
            }
            ')' => {
                out.push(Tk::RParen);
                i += 1;
            }
            ' ' | '\t' | '\n' | '\r' => {
                i += 1;
            }
            _ => return Err(()),
        }
    }
    Ok(out)
}

/// 递归下降求值：仅支持数字、+ - * / % ^ 与括号、一元 +/-。
fn eval_tokens(toks: &[Tk]) -> Result<f64, ()> {
    fn peek(toks: &[Tk], pos: &mut usize) -> Option<Tk> {
        toks.get(*pos).copied()
    }
    fn next(toks: &[Tk], pos: &mut usize) -> Option<Tk> {
        let t = toks.get(*pos).copied();
        if t.is_some() {
            *pos += 1;
        }
        t
    }
    fn parse_expr(toks: &[Tk], pos: &mut usize) -> Result<f64, ()> {
        let mut left = parse_term(toks, pos)?;
        while let Some(t) = peek(toks, pos) {
            if t == Tk::Plus || t == Tk::Minus {
                next(toks, pos);
                let right = parse_term(toks, pos)?;
                left = if t == Tk::Plus {
                    left + right
                } else {
                    left - right
                };
            } else {
                break;
            }
        }
        Ok(left)
    }
    fn parse_term(toks: &[Tk], pos: &mut usize) -> Result<f64, ()> {
        let mut left = parse_factor(toks, pos)?;
        while let Some(t) = peek(toks, pos) {
            if t == Tk::Star || t == Tk::Slash || t == Tk::Percent {
                next(toks, pos);
                let right = parse_factor(toks, pos)?;
                left = match t {
                    Tk::Star => left * right,
                    Tk::Slash => {
                        if right == 0.0 {
                            return Err(());
                        }
                        left / right
                    }
                    _ => {
                        if right == 0.0 {
                            return Err(());
                        }
                        left % right
                    }
                };
            } else {
                break;
            }
        }
        Ok(left)
    }
    fn parse_factor(toks: &[Tk], pos: &mut usize) -> Result<f64, ()> {
        // 处理右结合幂运算：**。
        let base = parse_unary(toks, pos)?;
        if let Some(Tk::Pow) = peek(toks, pos) {
            next(toks, pos);
            let exp = parse_factor(toks, pos)?;
            return Ok(base.powf(exp));
        }
        Ok(base)
    }
    fn parse_unary(toks: &[Tk], pos: &mut usize) -> Result<f64, ()> {
        if let Some(Tk::Minus) = peek(toks, pos) {
            next(toks, pos);
            return Ok(-parse_unary(toks, pos)?);
        }
        if let Some(Tk::Plus) = peek(toks, pos) {
            next(toks, pos);
            return parse_unary(toks, pos);
        }
        parse_primary(toks, pos)
    }
    fn parse_primary(toks: &[Tk], pos: &mut usize) -> Result<f64, ()> {
        match next(toks, pos) {
            Some(Tk::Num(n)) => Ok(n),
            Some(Tk::LParen) => {
                let v = parse_expr(toks, pos)?;
                match next(toks, pos) {
                    Some(Tk::RParen) => Ok(v),
                    _ => Err(()),
                }
            }
            _ => Err(()),
        }
    }
    let mut pos = 0usize;
    let v = parse_expr(toks, &mut pos)?;
    if pos != toks.len() {
        return Err(());
    }
    Ok(v)
}

fn safe_eval_math(expr: &str) -> Result<f64, ()> {
    let norm = expr
        .replace(['×', 'x', 'X'], "*")
        .replace('÷', "/")
        .replace('^', "**");
    let toks = tokenize(&norm)?;
    eval_tokens(&toks)
}

/// 把中文运算符词与符号统一转成 ASCII 运算符，供算术匹配/求值使用。
fn normalize_cn(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '×' | 'x' | 'X' => out.push('*'),
            '÷' => out.push('/'),
            '^' => out.push_str("**"),
            _ => out.push(ch),
        }
    }
    // 中文运算符词（长词在前，避免「乘以」被「乘」提前截断）。
    let mut s = out;
    for (cn, sym) in [
        ("乘以", "*"),
        ("除以", "/"),
        ("加上", "+"),
        ("减去", "-"),
        ("乘", "*"),
        ("除", "/"),
        ("加", "+"),
        ("减", "-"),
    ] {
        s = s.replace(cn, sym);
    }
    s
}

/// 从文本中抽出所有可安全求值的算术表达式（去重保序，排除纯数字）。
fn match_arithmetic(text: &str) -> Vec<String> {
    let norm = normalize_cn(text);
    let bytes = norm.as_bytes();
    let mut i = 0;
    let mut found: Vec<String> = Vec::new();
    while i < bytes.len() {
        let c = bytes[i] as char;
        if c.is_ascii_digit() || c == '.' || c == '(' || c == ')' {
            let start = i;
            let mut depth = 0i32;
            while i < bytes.len() {
                let ch = bytes[i] as char;
                if ch.is_ascii_digit()
                    || ch == '.'
                    || ch == '('
                    || ch == ')'
                    || ch == '+'
                    || ch == '-'
                    || ch == '*'
                    || ch == '/'
                    || ch == '%'
                    || ch == '^'
                    || ch == ' '
                {
                    if ch == '(' {
                        depth += 1;
                    } else if ch == ')' {
                        depth -= 1;
                    }
                    i += 1;
                } else {
                    break;
                }
            }
            let span = norm[start..i].trim().to_string();
            if depth == 0
                && span.contains(|c| "+-*/%^".contains(c))
                && safe_eval_math(&span).is_ok()
                && !contains_date_like(&span)
                && !found.contains(&span)
            {
                found.push(span);
            }
        } else {
            i += 1;
        }
    }
    found
}

/// 显式算术意图词：出现其一即认为用户在问一道计算题。
///
/// 仅收录多字安全词；故意不含单字「加/减/乘/除」，因为它们常出现在
/// 「减轻/乘法/除了」等非算术语境。归一化会把「3加4」转成纯算式，
/// 由 [`is_pure_expression`] 兜底放行，无需把单字算子当意图词。
const ARITH_CUES: &[&str] = &[
    "计算",
    "算一下",
    "算一算",
    "求值",
    "等于多少",
    "等于",
    "结果是",
    "结果等于",
    "答案是",
    "算出",
    "加上",
    "减去",
    "乘以",
    "除以",
    "加号",
    "减号",
    "乘号",
    "除号",
    "哪个更大",
    "哪个更小",
    "谁更",
    "比较大",
    "更大",
    "更小",
    "比一比",
    "compare",
];

/// 文本是否显含算术意图词。
fn has_arithmetic_cue(text: &str) -> bool {
    ARITH_CUES.iter().any(|kw| text.contains(kw))
}

/// 归一化后的文本若仍含任何字母/汉字，说明混入了自然语言或标识符，
/// 不是「纯算式」——此时不应作答，避免把订单号/版本号/邮编/电话等含
/// 连字符的标识符误判为减法（如 `8829-1234`）。
fn is_pure_expression(norm: &str) -> bool {
    !norm.chars().any(|c| c.is_alphabetic())
}

const COMPARE_KW: &[&str] = &[
    "哪个",
    "谁更",
    "比较大",
    "更大",
    "更小",
    "比一比",
    "compare",
];

pub(crate) fn try_arithmetic(text: &str) -> Option<FastAnswer> {
    let exprs = match_arithmetic(text);
    if exprs.is_empty() {
        return None;
    }
    // 严格门控：无显式算术意图词时，只有「无歧义纯算式」才作答。
    // 原文含连字符(-)即视为歧义——可能是减法，也可能是标识符/区间/电话/
    // 版本号/订单号，一律下沉 LLM；显式意图词（计算/等于…）出现时才按算式作答。
    // 注意：只看「原文」的 '-'，归一化由「减」合成的 '-' 属明确算式，不在此列。
    if !has_arithmetic_cue(text) {
        let norm = normalize_cn(text);
        let ambiguous = text.contains('-') || !is_pure_expression(&norm);
        if ambiguous {
            return None;
        }
    }
    let results: Vec<(String, f64)> = exprs
        .iter()
        .map(|e| {
            let v = safe_eval_math(e).unwrap_or(f64::NAN);
            (e.clone(), v)
        })
        .collect();
    let lines: Vec<String> = results
        .iter()
        .map(|(e, v)| format!("{e} = {}", format_number(*v)))
        .collect();
    let mut answer_lines = lines.clone();

    if results.len() >= 2 && COMPARE_KW.iter().any(|kw| text.contains(kw)) {
        let vals: Vec<f64> = results.iter().map(|(_, v)| *v).collect();
        if vals.iter().all(|v| *v == vals[0]) {
            answer_lines.push("两个结果相等".to_string());
        } else {
            let max_idx = vals
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
                .map(|(i, _)| i)
                .unwrap_or(0);
            answer_lines.push(format!(
                "其中 {} = {} 更大",
                results[max_idx].0,
                format_number(vals[max_idx])
            ));
        }
    }

    let answer = format!("，{}。", answer_lines.join("，"));
    Some(FastAnswer {
        method: "arithmetic",
        answer,
        detail: lines.join("\n"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eval_unsafe_rejected() {
        // 含非算术字符的表达式不应被当作可求值。
        assert!(safe_eval_math("import os").is_err());
    }

    #[test]
    fn identifier_like_hyphen_not_arithmetic() {
        // 订单号/版本号/邮编/电话 等含连字符的标识符不得被当成减法误算。
        assert!(try_arithmetic("我的订单号是 8829-1234，请尽快发货").is_none());
        assert!(try_arithmetic("版本 1.2-3 有点问题").is_none());
        assert!(try_arithmetic("邮编 100-0001").is_none());
        assert!(try_arithmetic("他的电话是 138-0013-1234").is_none());
        assert!(try_arithmetic("订单号8829-1234请尽快发货").is_none());
    }

    #[test]
    fn bare_hyphen_expression_is_ambiguous() {
        // 含连字符的裸表达式一律视为歧义，下沉 LLM（即便整段就是它自己）。
        assert!(try_arithmetic("8829-1234").is_none());
        assert!(try_arithmetic("100-200").is_none());
        assert!(try_arithmetic("10-2").is_none());
        // 显式意图词出现时才按算式作答。
        assert!(try_arithmetic("计算 10-2").is_some());
        // 归一化合成的 '-'（如「减」）属明确算式，不受歧义规则影响。
        assert!(try_arithmetic("5减3").is_some());
    }

    #[test]
    fn pure_or_cued_expression_still_works() {
        // 无意图词但整段就是算式（不含歧义连字符）：放行。
        assert!(try_arithmetic("12345 * 6789").is_some());
        assert!(try_arithmetic("3加4").is_some());
        assert!(try_arithmetic("1+1=?").is_some());
        assert!(try_arithmetic("(23+45)*2").is_some());
        // 显式意图词：即便夹在句子里也放行（含歧义连字符也照算）。
        assert!(try_arithmetic("计算 8829-1234 的结果").is_some());
        assert!(try_arithmetic("23 加 45 等于多少").is_some());
        assert!(try_arithmetic("哪个更大：2+3 还是 4*2").is_some());
    }
}
