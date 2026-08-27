//! 单位换算（温度/长度/重量/时间）。
use super::*;

// -- 单位换算 -------------------------------------------------------------------

struct Conversion {
    kw_a: &'static str,
    kw_b: &'static str,
    label: &'static str,
    target: &'static str,
    factor: f64,
}

const CONVERSIONS: &[Conversion] = &[
    Conversion {
        kw_a: "摄氏",
        kw_b: "华氏",
        label: "摄氏→华氏",
        target: "华氏度",
        factor: 0.0,
    },
    Conversion {
        kw_a: "华氏",
        kw_b: "摄氏",
        label: "华氏→摄氏",
        target: "摄氏度",
        factor: 0.0,
    },
    Conversion {
        kw_a: "公里",
        kw_b: "英里",
        label: "公里→英里",
        target: "英里",
        factor: 0.621371,
    },
    Conversion {
        kw_a: "英里",
        kw_b: "公里",
        label: "英里→公里",
        target: "公里",
        factor: 1.609344,
    },
    Conversion {
        kw_a: "千克",
        kw_b: "磅",
        label: "千克→磅",
        target: "磅",
        factor: 2.204623,
    },
    Conversion {
        kw_a: "公斤",
        kw_b: "磅",
        label: "千克→磅",
        target: "磅",
        factor: 2.204623,
    },
    Conversion {
        kw_a: "斤",
        kw_b: "千克",
        label: "斤→千克",
        target: "千克",
        factor: 0.5,
    },
    Conversion {
        kw_a: "小时",
        kw_b: "分钟",
        label: "小时→分钟",
        target: "分钟",
        factor: 60.0,
    },
    Conversion {
        kw_a: "分钟",
        kw_b: "小时",
        label: "分钟→小时",
        target: "小时",
        factor: 1.0 / 60.0,
    },
];

pub(crate) fn try_unit_convert(text: &str) -> Option<FastAnswer> {
    let nums = collect_numbers(text);
    let value = *nums.first()?;
    for c in CONVERSIONS {
        let has_a = text.contains(c.kw_a);
        let has_b = text.contains(c.kw_b);
        if !(has_a && has_b) {
            continue;
        }
        let result = if c.label == "摄氏→华氏" {
            value * 9.0 / 5.0 + 32.0
        } else if c.label == "华氏→摄氏" {
            (value - 32.0) * 5.0 / 9.0
        } else {
            value * c.factor
        };
        let display = format_number((result * 100.0).round() / 100.0);
        return Some(FastAnswer {
            method: "unit_convert",
            answer: format!("{value} 换算成 {target} 为 {display}。", target = c.target),
            detail: format!("{value} {label} = {result}", label = c.label),
        });
    }
    None
}
