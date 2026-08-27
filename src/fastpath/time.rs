//! 当前时间。
use super::*;
use chrono::{Datelike, Local};

// -- 当前时间 -------------------------------------------------------------------

pub(crate) fn try_time(text: &str) -> Option<FastAnswer> {
    const KW: &[&str] = &["现在几点", "几点了", "当前时间", "现在时间", "什么时间"];
    if !KW.iter().any(|k| text.contains(k)) && !text.to_lowercase().contains("time") {
        return None;
    }
    let now = Local::now();
    let w = WEEKDAYS[now.weekday().num_days_from_monday() as usize];
    let answer = format!(
        "现在是 {now}（{w}）。",
        now = now.format("%Y-%m-%d %H:%M:%S")
    );
    Some(FastAnswer {
        method: "time",
        answer,
        detail: now.to_rfc3339(),
    })
}
