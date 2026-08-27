/// 一个简单的 agent harness：把任务交给 LLM，由模型决定调用本地工具
/// （shell / 读写文件 / 列目录），工具结果再喂回模型，循环直到给出最终答案。
///
/// 架构参考 openai/codex 的「agent loop + tools + model client」三层，但刻意精简：
/// - `model`：OpenAI Responses API 的线格式类型
/// - `llm`：对 `/responses` 的 SSE 流式调用
/// - `tools`：在本地（沙箱）执行模型请求的工具
/// - `sandbox`：命令执行隔离（seatbelt / bwrap）
/// - `sessions`：git-stash 风格的会话存储（list / resolve）
/// - `agent`：驱动上述几者的主循环
mod agent;
// codegen（rhai 沙箱）与 tui（ratatui 界面）是两个可独立裁剪的重依赖模块：
// `--no-default-features` 或按需 `--features tui/codegen` 可显著加速构建。
#[cfg(feature = "codegen")]
pub mod codegen;
mod config;
mod error;
pub mod fastpath;
mod llm;
pub mod mcp;
mod memory;
mod model;
mod sandbox;
pub mod sessions;
pub mod skills;
mod tools;
#[cfg(feature = "tui")]
mod tui;

pub use agent::{AgentEvent, Conversation, run};
pub use config::Config;
pub use error::HarnessError;
pub use sandbox::SandboxPolicy;
#[cfg(feature = "tui")]
pub use tui::run_tui;
