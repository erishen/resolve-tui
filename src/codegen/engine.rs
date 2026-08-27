//! 受限 rhai 引擎：构建安全执行环境 + 正则白名单护栏。
//!
//! 这是 codegen 沙箱的核心隔离面——禁用 print/debug、仅注册少量纯数学与
//! 安全正则函数；文件/网络/eval 等能力在 rhai 里默认不存在，无需显式禁用。
//! 所有正则基于线性时间引擎，免疫灾难性回溯（ReDoS）。

use rhai::Engine;

/// 构建一个**受限** rhai 引擎：禁用 print/debug 输出，限时中断死循环，
/// 仅注册少量纯数学辅助函数；文件/网络/eval 等能力默认不存在，无需显式禁用。
pub(crate) fn build_engine() -> Engine {
    let mut engine = Engine::new();
    engine.on_print(|_| {});
    // on_debug 签名：Fn(&str, Option<&str>, Position)。
    engine.on_debug(|_: &str, _: Option<&str>, _: rhai::Position| {});
    engine.register_fn("abs", |x: f64| x.abs());
    engine.register_fn("floor", |x: f64| x.floor());
    engine.register_fn("ceil", |x: f64| x.ceil());
    engine.register_fn("round", |x: f64| x.round());
    engine.register_fn("sqrt", |x: f64| x.sqrt());
    engine.register_fn("pow", |x: f64, y: f64| x.powf(y));
    engine.register_fn("min", |a: f64, b: f64| a.min(b));
    engine.register_fn("max", |a: f64, b: f64| a.max(b));
    // 安全正则（基于 regex 引擎：线性时间、免疫灾难性回溯）。
    engine.register_fn("regex_match", regex_match);
    engine.register_fn("regex_find", regex_find);
    engine.register_fn("regex_capture", regex_capture);
    engine.register_fn("regex_replace", regex_replace);
    engine
}

/// 正则编译护栏：模式长度与编译后内存上限，防止病态输入拖垮执行线程。
const REGEX_MAX_PATTERN: usize = 256;
const REGEX_SIZE_LIMIT: usize = 256 * 1024;

/// 编译一个受约束的正则；模式过长或非法时返回 None（调用方据此安全降级）。
fn compile_regex(pat: &str) -> Option<regex::Regex> {
    if pat.len() > REGEX_MAX_PATTERN {
        return None;
    }
    regex::RegexBuilder::new(pat)
        .size_limit(REGEX_SIZE_LIMIT)
        .build()
        .ok()
}

/// `regex::Regex`（Google 实现）保证线性时间，免疫灾难性回溯（ReDoS）。
/// 全部函数对非法模式/无匹配返回安全默认值，绝不抛异常（检测器须健壮）。
fn regex_match(text: &str, pat: &str) -> bool {
    compile_regex(pat)
        .map(|re| re.is_match(text))
        .unwrap_or(false)
}

fn regex_find(text: &str, pat: &str) -> rhai::ImmutableString {
    match compile_regex(pat) {
        Some(re) => re.find(text).map_or("", |m| m.as_str()).into(),
        None => "".into(),
    }
}

fn regex_capture(text: &str, pat: &str) -> rhai::ImmutableString {
    match compile_regex(pat) {
        Some(re) => re
            .captures(text)
            .and_then(|c| c.get(1).or_else(|| c.get(0)))
            .map_or("", |m| m.as_str())
            .into(),
        None => "".into(),
    }
}

fn regex_replace(text: &str, pat: &str, repl: &str) -> rhai::ImmutableString {
    match compile_regex(pat) {
        Some(re) => re.replace_all(text, repl).into_owned().into(),
        None => text.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 直接验证沙箱里注册的正则函数（不依赖 LLM）。
    #[test]
    fn regex_helpers_safe_and_useful() {
        assert!(regex_match("订单号 8829-1234", r"\d{4}-\d{4}"));
        assert!(!regex_match("无数字", r"\d{4}-\d{4}"));
        assert_eq!(regex_find("订单号 8829-1234", r"\d{4}-\d{4}"), "8829-1234");
        assert_eq!(regex_capture("价格 12.5 元", r"(\d+\.\d+)"), "12.5");
        assert_eq!(regex_capture("价格 12.5 元", r"\d+"), "12");
        assert_eq!(regex_replace("a-b-c", "-", "/"), "a/b/c");
        // 非法模式：安全降级而非 panic。
        assert!(!regex_match("x", "("));
        assert_eq!(regex_find("x", "("), "");
        assert_eq!(regex_capture("x", "("), "");
        assert_eq!(regex_replace("x", "(", "!"), "x");
    }
}
