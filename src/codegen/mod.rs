//! 运行时代码生成：让模型为「fastpath 未命中」的问题写一段受限检测器，
//! 在 rhai 沙箱里执行，命中即作为答案，并持久化为插件供下次零模型复用。
//!
//! 设计对齐 `resolve-harness/src/resolve_harness/codegen.py`，但在 Rust 里把
//! 「Python 函数」替换为 **rhai 脚本**——rhai 是嵌入式安全脚本引擎，默认无
//! 文件/网络/FFI，且可通过注册白名单函数进一步收窄能力，天然对应原版的
//! AST 白名单 + 命名空间沙箱。生成器绝不可触达系统，失败只回退到普通 agent 循环。
//!
//! 子模块划分：
//! - `engine`：受限 rhai 引擎 + 正则白名单护栏
//! - `extract`：从模型回复抽取检测器源码
//! - `sandbox`：隔离执行（子进程协议 / 超时 kill / 逐源执行）
//! - `plugins`：插件缓存、命中统计、粗筛与上限淘汰，以及 codegen 主流程
//!
//! 本文件是纯 re-export 枢纽。

mod engine;
pub(crate) mod extract;
pub(crate) mod plugins;
pub(crate) mod sandbox;

pub use extract::extract_code;
pub use plugins::{
    PluginMeta, PluginStat, codegen_cached_answer, codegen_learn, codegen_plugin_dir,
    delete_plugin, list_plugins, save_plugin, try_codegen,
};
pub use sandbox::{CodegenAnswer, CodegenRunReq, detect_sources, run_codegen_child};
