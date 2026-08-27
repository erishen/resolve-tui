//! 进制转换（二/八/十六进制）。
use super::*;

// -- 进制转换 -------------------------------------------------------------------

pub(crate) fn try_base_convert(text: &str) -> Option<FastAnswer> {
    let nums = collect_numbers(text);
    let value = nums.first()?;
    let int_val = *value as i64;
    if text.contains("二进制") {
        return Some(FastAnswer {
            method: "base_convert",
            answer: format!("{int_val} 的二进制是 {:#b}。", int_val),
            detail: format!("{int_val} 的二进制是 {:#b}。", int_val),
        });
    }
    if text.contains("八进制") {
        return Some(FastAnswer {
            method: "base_convert",
            answer: format!("{int_val} 的八进制是 {:#o}。", int_val),
            detail: format!("{int_val} 的八进制是 {:#o}。", int_val),
        });
    }
    if text.contains("十六进制") {
        return Some(FastAnswer {
            method: "base_convert",
            answer: format!("{int_val} 的十六进制是 {:#x}。", int_val),
            detail: format!("{int_val} 的十六进制是 {:#x}。", int_val),
        });
    }
    None
}
