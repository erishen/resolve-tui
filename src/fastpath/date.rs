//! 日期计算（相差天数 / N 天后 / 明天昨天等）。
use super::*;

// -- 日期计算 -------------------------------------------------------------------

use chrono::{Datelike, Local};

fn parse_date(s: &str) -> Option<chrono::NaiveDate> {
    for sep in ["-", "/", "."] {
        if let Some((y, rest)) = s.split_once(sep)
            && let Some((m, d)) = rest.split_once(sep)
            && let (Ok(y), Ok(m), Ok(d)) = (y.parse::<i32>(), m.parse::<u32>(), d.parse::<u32>())
        {
            return chrono::NaiveDate::from_ymd_opt(y, m, d);
        }
    }
    None
}

fn weekday_cn(d: chrono::NaiveDate) -> &'static str {
    WEEKDAYS[d.weekday().num_days_from_monday() as usize]
}

pub(crate) fn try_date_math(text: &str) -> Option<FastAnswer> {
    let today = Local::now().date_naive();

    // 两日期相差天数：A 和 B 相差几天（直接扫出文本里所有日期，取前两个）。
    let dates: Vec<chrono::NaiveDate> = {
        let mut v = Vec::new();
        let mut i = 0;
        let chars: Vec<char> = text.chars().collect();
        while i < chars.len() {
            let mut seg = String::new();
            while i < chars.len() && (chars[i].is_ascii_digit() || ".-/".contains(chars[i])) {
                seg.push(chars[i]);
                i += 1;
            }
            if let Some(d) = parse_date(&seg) {
                v.push(d);
            }
            i += 1;
        }
        v
    };
    if text.contains("相差") && dates.len() >= 2 {
        let delta = (dates[1] - dates[0]).num_days().abs();
        return Some(FastAnswer {
            method: "date_math",
            answer: format!("{} 和 {} 相差 {} 天。", dates[0], dates[1], delta),
            detail: delta.to_string(),
        });
    }

    // N 天后 / 前（相对今天）。文本里若已含明确起始日期（如「2024-01-01 加 100 天」）
    // 则交给 LLM/codegen 处理，fastpath 不猜锚点，避免 false positive。
    if !contains_date_like(text)
        && let Some(idx) = text.find("天")
    {
        // 取「天」前最后一个整数作为偏移（跳过空格/标点），避免把其它数字算进来。
        let head = &text[..idx];
        let hb = head.as_bytes();
        let mut end = hb.len();
        while end > 0 && !hb[end - 1].is_ascii_digit() {
            end -= 1;
        }
        let mut start = end;
        while start > 0 && hb[start - 1].is_ascii_digit() {
            start -= 1;
        }
        if let Ok(n) = head[start..end].parse::<i64>() {
            let dir = if text[..idx].contains('减')
                || text[..idx].contains("减少")
                || text[idx..].starts_with("天前")
                || text[idx..].contains("之前")
            {
                -1
            } else {
                1
            };
            let target = if dir > 0 {
                today + chrono::Days::new(n as u64)
            } else {
                today - chrono::Days::new((-n) as u64)
            };
            return Some(FastAnswer {
                method: "date_math",
                answer: format!(
                    "{n} 天{d}是 {t}（{w}）。",
                    d = if dir > 0 { "后" } else { "前" },
                    t = target,
                    w = weekday_cn(target)
                ),
                detail: target.to_string(),
            });
        }
    }

    // 明天/昨天/后天/前天
    for (kw, off) in [("明天", 1), ("后天", 2), ("昨天", -1), ("前天", -2)] {
        if text.contains(kw) {
            let target = if off > 0 {
                today + chrono::Days::new(off as u64)
            } else {
                today - chrono::Days::new((-off) as u64)
            };
            return Some(FastAnswer {
                method: "date_math",
                answer: format!("{kw}是 {t}（{w}）。", t = target, w = weekday_cn(target)),
                detail: target.to_string(),
            });
        }
    }
    None
}
