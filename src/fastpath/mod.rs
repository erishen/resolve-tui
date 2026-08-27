//! 确定性快路径：用纯代码（零模型调用）直接回答某些查询。
//!
//! 设计对齐 `resolve-harness/src/resolve_harness/fastpath.py`——
//! 算术、单位换算、日期计算、进制转换、当前时间、数字统计等判定性问题
//! 完全可由代码求解，交给 LLM 反而慢、贵且易错。命中即短路返回，
//! 不进入 agent 主循环。
//!
//! 安全：所有求值都是手写递归下降解析器，绝不调用任何 `eval`/外部解释器；
//! 仅处理数字、运算符与括号，无法触达 IO/系统。

/// 一个确定性答案（绕过 LLM）。
#[derive(Debug, Clone)]
pub struct FastAnswer {
    /// 命中方法：`"arithmetic"` / `"unit_convert"` / `"date_math"` / ...
    pub method: &'static str,
    /// 给用户的最终回复（已含计算过程，可直接展示）。
    pub answer: String,
    /// 计算过程（供调试/展示）。
    pub detail: String,
}

// -- 数字抽取（共享） --------------------------------------------------------
// -- 数字抽取 -------------------------------------------------------------------

/// 从文本中抽出所有浮点数（按出现顺序）。
fn collect_numbers(text: &str) -> Vec<f64> {
    let mut out = Vec::new();
    let mut cur: String = String::new();
    let mut has_dot = false;
    for ch in text.chars() {
        if ch.is_ascii_digit() {
            cur.push(ch);
        } else if ch == '.' && !has_dot && !cur.is_empty() {
            has_dot = true;
            cur.push(ch);
        } else if !cur.is_empty() {
            if let Ok(n) = cur.parse::<f64>() {
                out.push(n);
            }
            cur.clear();
            has_dot = false;
        }
    }
    if !cur.is_empty()
        && let Ok(n) = cur.parse::<f64>()
    {
        out.push(n);
    }
    out
}

fn format_number(value: f64) -> String {
    if value.is_finite() && value.fract() == 0.0 && value.abs() < 1e15 {
        // 整数（且在 f64 精确表示范围内）直接去小数点。
        return (value as i64).to_string();
    }
    let s = format!("{value:.6}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

// -- 日期伪阳性守卫（共享） --------------------------------------------------
/// 判断文本是否含有「YYYY-MM-DD / YYYY-M-D」式日期片段（忽略空格）。
///
/// 用于收紧 fastpath：日期里的数字绝不能被当成算术/统计 operand 误判
/// （那是 false positive，会给出错误答案）。fastpath 的契约是「能精确算才开火」。
fn contains_date_like(s: &str) -> bool {
    let compact: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    let b = compact.as_bytes();
    let mut i = 0;
    while i + 4 < b.len() {
        if b[i].is_ascii_digit()
            && b[i + 1].is_ascii_digit()
            && b[i + 2].is_ascii_digit()
            && b[i + 3].is_ascii_digit()
            && b[i + 4] == b'-'
        {
            let mut j = i + 5;
            let mut m = 0;
            while j < b.len() && b[j].is_ascii_digit() {
                m += 1;
                j += 1;
            }
            if (1..=2).contains(&m) && j < b.len() && b[j] == b'-' {
                let mut k = j + 1;
                let mut d = 0;
                while k < b.len() && b[k].is_ascii_digit() {
                    d += 1;
                    k += 1;
                }
                if (1..=2).contains(&d) {
                    return true;
                }
            }
        }
        i += 1;
    }
    false
}

// -- 日历常量（共享） ------------------------------------------------------
const WEEKDAYS: &[&str] = &[
    "星期一",
    "星期二",
    "星期三",
    "星期四",
    "星期五",
    "星期六",
    "星期日",
];

// -- 公开入口 --------------------------------------------------------------
/// fastpath 只服务「短问题」：超过该字符数的一律下沉 LLM。
const MAX_INPUT_CHARS: usize = 80;

/// 当输入能被纯代码完整求解时返回 `FastAnswer`，否则 `None`。
///
/// `sandbox_dir` 预留给沙箱文件列举/读取类快路径（暂未实现），当前签名
/// 保持与 resolve-harness 对齐。
pub fn try_fast_answer(text: &str, _sandbox_dir: Option<&str>) -> Option<FastAnswer> {
    if text.trim().is_empty() {
        return None;
    }
    // 长输入直接放弃：fastpath 只服务「短问题」。多步骤任务描述/清单几乎
    // 不可能是纯计算题，但里面容易顺带出现『排序』『现在几点』等关键词，
    // 造成答非所问的 false positive——宁可漏判（下沉 LLM），绝不错判。
    if text.chars().count() > MAX_INPUT_CHARS {
        return None;
    }
    let checks: &[fn(&str) -> Option<FastAnswer>] = &[
        try_arithmetic,
        try_statistics,
        try_unit_convert,
        try_date_math,
        try_base_convert,
        try_time,
    ];
    for check in checks {
        if let Some(ans) = check(text) {
            return Some(ans);
        }
    }
    None
}

mod arithmetic;
mod base;
mod date;
mod statistics;
mod time;
mod unit;

use arithmetic::try_arithmetic;
use base::try_base_convert;
use date::try_date_math;
use statistics::try_statistics;
use time::try_time;
use unit::try_unit_convert;

#[cfg(test)]
mod tests {
    use super::*;

    fn ans(text: &str) -> Option<String> {
        try_fast_answer(text, None).map(|a| a.answer)
    }

    #[test]
    fn arithmetic_basic() {
        assert!(ans("计算 2+3").unwrap().contains("2+3 = 5"));
        assert!(ans("12×34 等于多少").unwrap().contains("408"));
        assert!(ans("(23+45)*2").unwrap().contains("136"));
    }

    #[test]
    fn arithmetic_cn_ops() {
        assert!(ans("23 加 45").unwrap().contains("68"));
        assert!(ans("100 除以 4").unwrap().contains("25"));
    }

    #[test]
    fn arithmetic_compare() {
        let a = ans("哪个更大：2+3 还是 4*2").unwrap();
        assert!(a.contains("更大"));
    }

    #[test]
    fn not_arithmetic_plain_text() {
        assert!(ans("帮我写一首诗").is_none());
    }

    #[test]
    fn unit_convert() {
        let a = ans("100 摄氏度 转 华氏度").unwrap();
        assert!(a.contains("212"));
        let b = ans("1 公里 等于 几 英里").unwrap();
        assert!(b.contains("0.62") || b.contains("0.621"));
    }

    #[test]
    fn date_math() {
        assert!(ans("明天是星期几").unwrap().contains("星期"));
        assert!(ans("100 天后是几号").is_some());
    }

    #[test]
    fn date_not_misread_as_arithmetic() {
        // 曾经被 arithmetic 误算成 2122（false positive）；收紧后应下沉到 LLM。
        assert!(ans("2024-01-01 加 100 天是哪天").is_none());
    }

    #[test]
    fn order_number_not_misread_as_arithmetic() {
        // 回归：订单号 8829-1234 曾被误算成减法 7595（false positive）。
        // 收紧后整段不是纯算式也没有算术意图词，应下沉 LLM 而非快路径作答。
        assert!(ans("我的订单号是 8829-1234，请尽快发货").is_none());
        // 裸订单号本身含歧义连字符，同样下沉 LLM。
        assert!(ans("8829-1234").is_none());
    }

    #[test]
    fn date_not_misread_as_statistics() {
        assert!(ans("2024-01-01 和 2025-03-04 哪个最大").is_none());
    }

    #[test]
    fn day_offset_ignores_other_numbers() {
        // 「天」前紧邻的整数才是偏移，不应把前面的金额数字算进来。
        assert!(ans("我有100块钱，30天后是几号").is_some());
    }

    #[test]
    fn long_task_list_not_hijacked_by_statistics() {
        // 真实回归：任务清单里的序号 1.2.3.4 + 顺带的『按优先级排序』，
        // 曾被 statistics 劫持答成「从小到大为 1、2、3、4、500」。
        let q = "全面体检这个仓库：1. 列出目录结构 2. 找出超过 500 行的源码文件 \
                 3. 检查滥用 4. 给一份按优先级排序的重构建议表";
        assert!(ans(q).is_none());
    }

    #[test]
    fn stats_keyword_far_from_numbers_is_ignored() {
        assert!(ans("先处理 1 和 2 两件事，最后输出按优先级排序的方案").is_none());
        // 关键词紧邻数字的正常用例不受影响。
        assert!(ans("5 和 8 哪个最大").is_some());
        assert!(ans("1、2、3 的平均值").is_some());
    }

    #[test]
    fn overlong_input_skips_fastpath_entirely() {
        let filler = "这是一段很长的铺垫。".repeat(8); // >80 字符
        let q = format!("{filler}计算 2+3");
        assert!(q.chars().count() > 80);
        assert!(ans(&q).is_none(), "超长输入应整体下沉 LLM");
    }

    #[test]
    fn base_convert() {
        assert!(ans("10 的二进制").unwrap().contains("1010"));
        assert!(ans("255 的十六进制").unwrap().contains("ff"));
    }

    #[test]
    fn time_now() {
        assert!(ans("现在几点").unwrap().contains("现在"));
    }

    #[test]
    fn statistics() {
        assert!(ans("1、2、3 的平均值").unwrap().contains("2"));
        assert!(ans("5 和 8 哪个最大").unwrap().contains("8"));
    }
}
