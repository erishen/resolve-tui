//! 环境变量解析：字符串（`env_or`）、开关（`env_flag`）、目录白名单（`parse_roots`）。

use std::path::PathBuf;

use crate::sandbox::SandboxPolicy;

/// 读取环境变量，缺失或为空时回退到 `default`。
pub(crate) fn env_or(key: &str, default: &str) -> String {
    std::env::var(key)
        .map(|v| if v.is_empty() { default.to_string() } else { v })
        .unwrap_or_else(|_| default.to_string())
}

/// 解析开关型环境变量：`true`/`1`/`yes` 为真，否则用 `default`。
pub(crate) fn env_flag(key: &str, default: bool) -> bool {
    match std::env::var(key) {
        Ok(v) => matches!(v.to_lowercase().as_str(), "1" | "true" | "yes" | "on"),
        Err(_) => default,
    }
}

/// 解析逗号分隔的白名单目录；为空时回退到沙箱默认根（当前目录 + 临时目录）。
pub(crate) fn parse_roots(key: &str) -> Vec<PathBuf> {
    match std::env::var(key) {
        Ok(v) if !v.trim().is_empty() => v
            .split(',')
            .map(|s| PathBuf::from(s.trim()))
            .filter(|p| !p.as_os_str().is_empty())
            .collect(),
        _ => SandboxPolicy::default_roots(),
    }
}
