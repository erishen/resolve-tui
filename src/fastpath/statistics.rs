//! 数字统计（最大/最小/平均/求和/排序），含 false-positive 收紧。
use super::*;

// -- 数字统计 -------------------------------------------------------------------

/// 统计关键词表（按优先级）：关键词 → (计算种类, 答案标签)。
const STAT_KINDS: &[(&str, &str, &str)] = &[
    ("最大", "max", "最大值为"),
    ("最小", "min", "最小值为"),
    ("平均", "avg", "平均值为"),
    ("总和", "sum", "总和为"),
    ("求和", "sum", "总和为"),
    ("排序", "sort", "从小到大为"),
    ("从小到大", "sort", "从小到大为"),
    ("从大到小", "sort_desc", "从大到小为"),
];

/// 统计关键词与最近数字的最大字符距离：超过视为「顺带提及」而非统计意图。
const STAT_KW_MAX_DIST: usize = 12;

/// 数字必须构成「一个数据簇」：相邻数字间隔超过该字符数即视为散布全文的
/// 清单序号/无关数字，而非统计数据。真实回归：『…1. 列出…2. 找出 500 行…
/// 3. …4. 按…排序…』各序号间隔 8~11 字符，被顺带的『排序』劫持过。
const STAT_CLUSTER_GAP: usize = 8;

/// 找到第一个「紧邻数字簇」的统计关键词；没有则 `None`。
fn stat_keyword_near_numbers(
    text: &str,
    num_pos: &[usize],
) -> Option<(&'static str, &'static str)> {
    if num_pos.is_empty() {
        return None;
    }
    STAT_KINDS.iter().find_map(|(kw, kind, label)| {
        let byte_idx = text.find(kw)?;
        let kw_char = text[..byte_idx].chars().count();
        let nearest = num_pos
            .iter()
            .map(|&n| kw_char.abs_diff(n))
            .min()
            .unwrap_or(usize::MAX);
        (nearest <= STAT_KW_MAX_DIST).then_some((*kind, *label))
    })
}

/// 抽出所有数字起始位置的字符下标（供「关键词紧邻数字」判定）。
fn number_positions(chars: &[char]) -> Vec<usize> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_ascii_digit() {
            out.push(i);
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    out
}

pub(crate) fn try_statistics(text: &str) -> Option<FastAnswer> {
    let nums = collect_numbers(text);
    if nums.len() < 2 {
        return None;
    }
    // 日期里的数字不应参与统计，否则「2024-01-01 和 2025-03-04 哪个最大」会答错。
    if contains_date_like(text) {
        return None;
    }
    // 数字必须聚成一簇：散布全文的是任务清单序号，不是统计数据。
    let chars: Vec<char> = text.chars().collect();
    let num_pos = number_positions(&chars);
    if num_pos.len() >= 2 && num_pos.windows(2).any(|w| w[1] - w[0] > STAT_CLUSTER_GAP) {
        return None;
    }
    // 关键词必须「紧邻」数字簇：『1、2、3 的平均值』距离 3 ✓；
    // 长句里顺带出现『按优先级排序』则远超阈值 ✗。
    let (kind, label) = stat_keyword_near_numbers(text, &num_pos)?;
    let value = match kind {
        "max" => *nums.iter().fold(&nums[0], |a, b| if b > a { b } else { a }),
        "min" => *nums.iter().fold(&nums[0], |a, b| if b < a { b } else { a }),
        "avg" => nums.iter().sum::<f64>() / nums.len() as f64,
        "sum" => nums.iter().sum::<f64>(),
        "sort" => {
            let mut s = nums.clone();
            s.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let joined = s
                .iter()
                .map(|v| format_number(*v))
                .collect::<Vec<_>>()
                .join("、");
            return Some(FastAnswer {
                method: "statistics",
                answer: format!("{label} {joined}。"),
                detail: joined,
            });
        }
        _ => {
            let mut s = nums.clone();
            s.sort_by(|a, b| b.partial_cmp(a).unwrap());
            let joined = s
                .iter()
                .map(|v| format_number(*v))
                .collect::<Vec<_>>()
                .join("、");
            return Some(FastAnswer {
                method: "statistics",
                answer: format!("{label} {joined}。"),
                detail: joined,
            });
        }
    };
    Some(FastAnswer {
        method: "statistics",
        answer: format!("{label} {v}。", v = format_number(value)),
        detail: format_number(value),
    })
}
