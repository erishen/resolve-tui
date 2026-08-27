//! 插件治理与「公开入口」：缓存加载、命中统计、粗筛与上限淘汰、持久化，
//! 以及把缓存命中 + 模型生成拼接起来的 codegen 主流程（codegen_solve）。
//!
//! 拆分为：
//! - [`cache`]：插件缓存、命中统计、粗筛与上限淘汰
//! - [`registry`]：插件持久化（列出/删除/保存）
//! - [`learn`]：codegen 主流程（缓存查找 / 事后学习 / 组合入口）

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// 生成器系统提示：约束输出为单个 rhai `detect` 函数，并列出可用安全能力。
const CODE_GEN_SYSTEM: &str = r##"你是 fast-path 代码生成器。判断下面的用户问题能否用**纯 rhai 脚本**确定性解决（字符串/数字/列表处理、日期、单位换算、数学计算——不需要网络、文件或外部库）。

如果能解决，只输出一个 rhai 函数（不要解释、不要多余文字）：

fn detect(text) {
    // 从 text 取出所需信息，返回完整自然的答案字符串（中文）；
    // 如果 text 不是这类问题，返回空串 ""。
    if text.contains("ping") { "pong" } else { "" }
}

注意：rhai 是动态类型，函数参数与返回值**不要写类型标注**（不要写 `: String`、`-> String`）。

规则：
- 只能使用：算术运算符 + - * / %、字符串方法（contains/starts_with/ends_with/trim/replace/split/sub_string/to_upper/to_lower/len）、数字解析（to_int/to_float）、数学函数 abs/floor/ceil/round/sqrt/pow/min/max，以及我额外提供的安全正则函数：
  - `regex_match(text, pat) -> bool`：是否匹配
  - `regex_find(text, pat) -> String`：整段匹配（无则 ""）
  - `regex_capture(text, pat) -> String`：首个捕获组（无捕获组则整段匹配，无匹配则 ""）
  - `regex_replace(text, pat, repl) -> String`：替换全部
  正则基于线性时间引擎，无 ReDoS 风险；模式非法时一律安全返回（false / "" / 原文）。
- rhai 的**原始字符串**用 `#"..."#` 语法（不是 Rust 的 `r"..."`），写正则最方便，例如 `#"(\d+)\s*加\s*(\d+)"#`；普通 `"..."` 里的反斜杠要写成 `\\`。
- 禁止任何文件读写、网络访问、系统调用、eval/exec、外部模块 import——引擎默认就不提供这些能力。
- 函数必须健壮：对不相关输入返回 ""，绝不抛异常（可用 try/catch）。
- 答案要完整自然，例如「2 加 3 等于 5。」。

如果不能确定性解决（需要常识、写作、开放推理、工具调用）→ 只输出 NONE。"##;

/// 插件目录：`<config_dir>/resolve-tui/codegen_plugins`；无法确定时回退 cwd。
fn default_plugin_dir() -> PathBuf {
    dirs::config_dir()
        .map(|d| d.join("resolve-tui").join("codegen_plugins"))
        .unwrap_or_else(|| PathBuf::from("codegen_plugins"))
}

/// 返回默认 codegen 插件目录（供 CLI 管理子命令复用）。
pub fn codegen_plugin_dir() -> PathBuf {
    default_plugin_dir()
}

/// 由源码算出的稳定文件名（16 位 hex），相同源码去重到同一文件（幂等）。
fn plugin_name(source: &str) -> String {
    let mut h = DefaultHasher::new();
    source.hash(&mut h);
    format!("gen_{:016x}", h.finish())
}

fn mtime_secs(path: &Path) -> i64 {
    std::fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 当前 Unix 时间戳（秒）；时钟异常时为 0。
fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 已加载的插件：文件名（即插件名）与源码。
/// 生产路径把源码送子进程执行；父进程不保留 AST（避免每次热重载重复编译）。
pub(crate) struct LoadedPlugin {
    pub name: String,
    pub src: String,
}

mod cache;
mod learn;
mod registry;

#[cfg(test)]
mod tests;

// 公开 API（与 `codegen::*` 再导出保持一致）。
pub use cache::PluginStat;
pub use learn::{codegen_cached_answer, codegen_learn, try_codegen};
pub use registry::{PluginMeta, delete_plugin, list_plugins, save_plugin};
