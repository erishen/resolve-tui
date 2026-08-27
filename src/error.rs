/// harness 内部统一错误类型。
///
/// 所有模块的内部失败都应收敛到这一类型，避免在签名里散落 `Result<_, String>`
/// 或各自定义局部错误类型（见审计 P2-8）。顶层 [`crate::agent::Conversation::submit`]
/// 与 CLI 入口 [`crate::agent::run`] 也以它为返回类型，形成单一错误出口。
#[derive(Debug, thiserror::Error)]
pub enum HarnessError {
    #[error("llm error: {0}")]
    Llm(String),
    #[error("tool error: {0}")]
    Tool(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("达到最大迭代次数 {0}")]
    MaxIterations(usize),
    #[error("已取消")]
    Cancelled,
    /// 配置校验 / 读写失败（config.toml、钥匙串、MCP server 段增删等）。
    #[error("config error: {0}")]
    Config(String),
    /// 其它内部错误兜底（记忆落盘、插件持久化、codegen 沙箱等不归入上述分类者）。
    #[error("{0}")]
    Other(String),
}

impl HarnessError {
    pub fn llm(msg: impl Into<String>) -> Self {
        Self::Llm(msg.into())
    }

    pub fn tool(msg: impl Into<String>) -> Self {
        Self::Tool(msg.into())
    }

    pub fn max_iterations(n: usize) -> Self {
        Self::MaxIterations(n)
    }

    pub fn cancelled() -> Self {
        Self::Cancelled
    }

    pub fn config(msg: impl Into<String>) -> Self {
        Self::Config(msg.into())
    }

    pub fn other(msg: impl Into<String>) -> Self {
        Self::Other(msg.into())
    }
}
