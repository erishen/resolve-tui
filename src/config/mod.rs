//! 配置加载：`Config` 结构体、默认值、TOML/环境变量合并与校验。
//!
//! 文件读写（config.toml 定位与 MCP server 段增删）见 `file` 子模块；
//! 环境变量解析（字符串/开关/目录白名单）见 `env` 子模块。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use serde::Deserialize;

use crate::{HarnessError, mcp::McpServerConfig, sandbox::SandboxPolicy};

mod env;
mod file;

#[cfg(test)]
mod tests;

// 供 `load` / `apply_env`（本模块内）按名调用的辅助函数。
pub(crate) use env::{env_flag, env_or, parse_roots};
pub(crate) use file::read_toml;
// 对外公开的配置读写（TUI 的 /mcp add|remove 会持久化到 config.toml）。
// `append/remove` 仅在 tui 特性下被调用，关闭时作为公开 API 保留（测试仍引用），
// 故特例放行其未使用告警。
pub use file::config_file;
#[cfg_attr(not(feature = "tui"), allow(unused_imports))]
pub use file::{append_mcp_server, remove_mcp_server};

/// 读取运行配置，全部可经环境变量覆盖。
#[derive(Clone, Debug)]
pub struct Config {
    /// 模型名（传给 `/responses` 的 `model`）。
    pub model: String,
    /// OpenAI 兼容 API 的 base url，不含末尾 `/responses`。
    pub api_base: String,
    /// API key（bearer token）。
    pub api_key: String,
    /// agent 循环的最大迭代次数，防止模型死循环。
    pub max_iterations: usize,
    /// 工具执行沙箱策略。
    pub policy: SandboxPolicy,
    /// 沙箱根目录（每任务在此下建独立工作区）。默认 `<cwd>/.resolve-tui-sandbox`，
    /// 可用 `HARNESS_SANDBOX_DIR` 覆盖。
    pub sandbox_dir: PathBuf,
    /// 是否使用 Responses API 的 `previous_response_id` 有状态续接。
    /// 部分网关（如 Agnes）不持久化响应，置 false 时改为无状态回灌 `function_call` 项。
    pub stateful: bool,
    /// 调试开关：强制模型至少调用一个工具（走 `tool_choice: "required"`）。
    pub force_tools: bool,
    /// 工具审批模式：每次工具调用前需用户确认（仅交互式 TUI 生效）。
    pub approve_tools: bool,
    /// 多 Agent 模式：以 agentic-souls 的三角色（Planner/Specialist/Evaluator）
    /// 编排，取代默认的单 agent 循环。默认关闭，由 `HARNESS_MULTI_AGENT` 或
    /// `--multi-agent` 开启。
    pub multi_agent: bool,
    /// 是否启用 codegen 快路径：fastpath 未命中时，让模型生成一段受限脚本
    /// 检测器（rhai 沙箱）直接求解并持久化，下次同类问题零模型命中。
    pub codegen: bool,
    /// codegen 学习/生成使用的模型；`None` 沿用主模型。分层路由：
    /// 检测器生成是结构化小任务，适合便宜快速的模型（env `HARNESS_CODEGEN_MODEL`）。
    pub codegen_model: Option<String>,
    /// 累计 token 预算上限；>0 时超出即终止 agent 循环，0 表示不限。
    pub max_tokens: u64,
    /// 无状态模式下每次请求发送的最大历史条数；超出时从最近的 user 消息边界
    /// 截断（本地存档仍保留全量，只裁剪发给模型的部分）。0 表示不限制。
    pub history_max_items: usize,
    /// codegen 插件目录；`None` 用系统配置目录下的默认位置。按项目隔离插件时
    /// 显式指定（如 `.resolve-tui/plugins`），env `HARNESS_CODEGEN_DIR` 可覆盖。
    pub codegen_plugin_dir: Option<PathBuf>,
    /// 界面配色主题：`dark`（默认，黑底）或 `light`（白底，避免文字看不清）。
    pub theme: String,
    /// MCP server 启动配置；按配置文件里的声明顺序（BTreeMap 保证确定性）。
    pub mcp_servers: Vec<McpServerConfig>,
}

/// 配置文件（TOML）的结构；所有字段可选，缺失项回退到默认值，环境变量最终覆盖。
/// 注意：布尔开关在文件里用普通布尔，环境变量仍用 `1/true/yes/on`。
#[derive(Debug, Default, Deserialize)]
pub(crate) struct TomlConfig {
    model: Option<String>,
    api_base: Option<String>,
    api_key: Option<String>,
    max_iterations: Option<usize>,
    stateful: Option<bool>,
    force_tools: Option<bool>,
    approve_tools: Option<bool>,
    multi_agent: Option<bool>,
    codegen: Option<bool>,
    codegen_model: Option<String>,
    max_tokens: Option<u64>,
    history_max_items: Option<usize>,
    codegen_plugin_dir: Option<String>,
    theme: Option<String>,
    sandbox_enabled: Option<bool>,
    sandbox_allow_network: Option<bool>,
    sandbox_roots: Option<Vec<String>>,
    sandbox_dir: Option<String>,
    mcp_servers: Option<BTreeMap<String, McpServerToml>>,
}

/// `[mcp_servers.<name>]` 单项。
#[derive(Debug, Deserialize)]
struct McpServerToml {
    command: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    /// 单次 tools/call 超时（秒）；缺省用全局默认（120s）。长任务 server
    /// （如 pse-review，2-6 分钟）务必指定，否则会被超时打断。
    #[serde(default)]
    timeout_secs: Option<u64>,
}

impl Config {
    /// 完整加载：默认值 → 配置文件（若存在）→ 环境变量覆盖。
    pub fn load() -> Arc<Self> {
        let mut cfg = Self::defaults();
        if let Some(path) = config_file()
            && let Some(toml_cfg) = read_toml(&path)
        {
            cfg.apply_toml(&toml_cfg);
        }
        cfg.apply_env();
        Arc::new(cfg)
    }

    /// 从环境变量构造，缺失时使用本地兜底默认值（便于离线调试结构）。
    pub fn from_env() -> Arc<Self> {
        let mut cfg = Self::defaults();
        cfg.apply_env();
        Arc::new(cfg)
    }

    /// 配置合理性校验：模型名非空、API base 为合法 http(s) URL、迭代次数与预算合法。
    /// 返回 `Err` 时给出可读的中文原因；用于启动前告警（非致命，不阻断运行）。
    pub fn validate(&self) -> Result<(), HarnessError> {
        if self.model.trim().is_empty() {
            return Err(HarnessError::config("model 不能为空"));
        }
        if !self.api_base.starts_with("http://") && !self.api_base.starts_with("https://") {
            return Err(HarnessError::config(format!(
                "api_base 必须是 http(s) URL，当前为：{}",
                self.api_base
            )));
        }
        if self.api_base.ends_with('/') {
            return Err(HarnessError::config(format!(
                "api_base 不应以 / 结尾（调用时会自动拼接 /responses）：{}",
                self.api_base
            )));
        }
        if self.max_iterations == 0 {
            return Err(HarnessError::config("max_iterations 必须大于 0"));
        }
        if !self.theme.is_empty() && !["dark", "light", "auto"].contains(&self.theme.as_str()) {
            return Err(HarnessError::config(format!(
                "theme 仅允许 dark/light/auto，当前为：{}",
                self.theme
            )));
        }
        Ok(())
    }

    /// 兜底默认值（不含任何外部来源）。
    fn defaults() -> Self {
        Self {
            api_key: String::new(),
            api_base: "https://api.openai.com/v1".to_string(),
            model: "gpt-4o-mini".to_string(),
            max_iterations: 16,
            policy: SandboxPolicy {
                enabled: true,
                allow_network: false,
                writable_roots: SandboxPolicy::default_roots(),
                cwd: None,
            },
            sandbox_dir: std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(".resolve-tui-sandbox"),
            stateful: false,
            force_tools: false,
            approve_tools: false,
            multi_agent: false,
            codegen: true,
            codegen_model: None,
            max_tokens: 0,
            history_max_items: 200,
            codegen_plugin_dir: None,
            theme: "auto".to_string(),
            mcp_servers: Vec::new(),
        }
    }

    /// 把配置文件里的字段并入（仅覆盖显式设置的项）。
    /// 所有覆盖规则放在一张表里，新增配置项只需往 `table` 加一行，避免散落的 `if let Some` 样板。
    fn apply_toml(&mut self, t: &TomlConfig) {
        type Set = Box<dyn Fn(&mut Config, &TomlConfig)>;
        let table: &[Set] = &[
            Box::new(|c, t| {
                if let Some(v) = &t.model {
                    c.model = v.clone();
                }
            }),
            Box::new(|c, t| {
                if let Some(v) = &t.api_base {
                    c.api_base = v.clone();
                }
            }),
            Box::new(|c, t| {
                if let Some(v) = &t.api_key {
                    c.api_key = v.clone();
                }
            }),
            Box::new(|c, t| {
                if let Some(v) = t.max_iterations {
                    c.max_iterations = v;
                }
            }),
            Box::new(|c, t| {
                if let Some(v) = t.stateful {
                    c.stateful = v;
                }
            }),
            Box::new(|c, t| {
                if let Some(v) = t.force_tools {
                    c.force_tools = v;
                }
            }),
            Box::new(|c, t| {
                if let Some(v) = t.approve_tools {
                    c.approve_tools = v;
                }
            }),
            Box::new(|c, t| {
                if let Some(v) = t.multi_agent {
                    c.multi_agent = v;
                }
            }),
            Box::new(|c, t| {
                if let Some(v) = t.codegen {
                    c.codegen = v;
                }
            }),
            Box::new(|c, t| {
                if let Some(v) = &t.codegen_model {
                    c.codegen_model = Some(v.clone());
                }
            }),
            Box::new(|c, t| {
                if let Some(v) = t.max_tokens {
                    c.max_tokens = v;
                }
            }),
            Box::new(|c, t| {
                if let Some(v) = t.history_max_items {
                    c.history_max_items = v;
                }
            }),
            Box::new(|c, t| {
                if let Some(v) = &t.codegen_plugin_dir {
                    c.codegen_plugin_dir = Some(PathBuf::from(v));
                }
            }),
            Box::new(|c, t| {
                if let Some(v) = &t.theme {
                    c.theme = v.clone();
                }
            }),
            // 沙箱策略：三个子字段任一出现就整体重算，未出现项沿用当前值。
            Box::new(|c, t| {
                if t.sandbox_enabled.is_some()
                    || t.sandbox_allow_network.is_some()
                    || t.sandbox_roots.is_some()
                {
                    c.policy = SandboxPolicy {
                        enabled: t.sandbox_enabled.unwrap_or(c.policy.enabled),
                        allow_network: t.sandbox_allow_network.unwrap_or(c.policy.allow_network),
                        writable_roots: t
                            .sandbox_roots
                            .clone()
                            .map(|rs| rs.into_iter().map(PathBuf::from).collect())
                            .unwrap_or_else(|| c.policy.writable_roots.clone()),
                        cwd: c.policy.cwd.clone(),
                    };
                }
            }),
            Box::new(|c, t| {
                if let Some(v) = &t.sandbox_dir {
                    c.sandbox_dir = PathBuf::from(v);
                }
            }),
            // MCP server：按声明顺序（BTreeMap 已排序）展开为 Vec。
            Box::new(|c, t| {
                if let Some(servers) = &t.mcp_servers {
                    c.mcp_servers = servers
                        .iter()
                        .map(|(name, s)| McpServerConfig {
                            name: name.clone(),
                            command: s.command.clone(),
                            args: s.args.clone(),
                            env: s.env.clone().into_iter().collect(),
                            call_timeout: s
                                .timeout_secs
                                .map(std::time::Duration::from_secs)
                                .unwrap_or_default(),
                        })
                        .collect();
                }
            }),
        ];
        for set in table {
            set(self, t);
        }
    }

    /// 解析 API key 的来源优先级：
    /// 1. 环境变量 `OPENAI_API_KEY`（最直接，便于临时覆盖）；
    /// 2. 系统钥匙串（macOS Keychain / 各平台 secret-service），避免把密钥明文写进 `.env` 或 config.toml；
    /// 3. 都为空则返回空串（调用方会在 `validate` 时告警）。
    ///
    /// 钥匙串读取失败时静默回退，不阻断启动。写入钥匙串可用 `store_api_key`，
    /// 或 macOS 手动：`security add-generic-password -s resolve-tui -a openai-api-key -w <key>`。
    fn resolve_api_key() -> String {
        if let Ok(v) = std::env::var("OPENAI_API_KEY")
            && !v.is_empty()
        {
            return v;
        }
        if let Ok(entry) = keyring::Entry::new("resolve-tui", "openai-api-key")
            && let Ok(v) = entry.get_password()
            && !v.is_empty()
        {
            return v;
        }
        String::new()
    }

    /// 将 API key 持久化到系统钥匙串，便于后续启动不再依赖明文 `.env` / config.toml。
    pub fn store_api_key(key: &str) -> Result<(), HarnessError> {
        keyring::Entry::new("resolve-tui", "openai-api-key")
            .map_err(|e| HarnessError::config(format!("无法访问系统钥匙串：{e}")))?
            .set_password(key)
            .map_err(|e| HarnessError::config(format!("写入钥匙串失败：{e}")))
    }

    /// 用环境变量覆盖（优先级最高）。
    /// 每条覆盖规则放在一张表里，新增环境变量只需往 `table` 加一行；字符串类用
    /// `env_or`（缺失则保持现状）、布尔类用 `env_flag`、数值类解析失败则跳过，
    /// 与原有逐字段逻辑完全等价。
    fn apply_env(&mut self) {
        type Set = Box<dyn Fn(&mut Config)>;
        let table: &[Set] = &[
            // API key 走专门的解析（环境变量 > 系统钥匙串）。
            Box::new(|c| {
                let k = Config::resolve_api_key();
                if !k.is_empty() {
                    c.api_key = k;
                }
            }),
            Box::new(|c| c.api_base = env_or("OPENAI_API_BASE", &c.api_base)),
            Box::new(|c| c.model = env_or("HARNESS_MODEL", &c.model)),
            Box::new(|c| {
                if let Some(v) = std::env::var("HARNESS_MAX_ITERATIONS")
                    .ok()
                    .and_then(|s| s.parse::<usize>().ok())
                {
                    c.max_iterations = v;
                }
            }),
            Box::new(|c| {
                if std::env::var("HARNESS_SANDBOX").is_ok() {
                    c.policy.enabled = env_flag("HARNESS_SANDBOX", c.policy.enabled);
                }
            }),
            Box::new(|c| {
                if std::env::var("HARNESS_ALLOW_NETWORK").is_ok() {
                    c.policy.allow_network =
                        env_flag("HARNESS_ALLOW_NETWORK", c.policy.allow_network);
                }
            }),
            Box::new(|c| {
                if std::env::var("HARNESS_SANDBOX_ROOTS").is_ok() {
                    c.policy.writable_roots = parse_roots("HARNESS_SANDBOX_ROOTS");
                }
            }),
            // 指定沙箱根目录才覆盖（空串不清除已有值）。
            Box::new(|c| {
                if let Ok(v) = std::env::var("HARNESS_SANDBOX_DIR")
                    && !v.trim().is_empty()
                {
                    c.sandbox_dir = PathBuf::from(v.trim());
                }
            }),
            Box::new(|c| {
                if std::env::var("HARNESS_STATEFUL").is_ok() {
                    c.stateful = env_flag("HARNESS_STATEFUL", c.stateful);
                }
            }),
            Box::new(|c| {
                if std::env::var("HARNESS_FORCE_TOOLS").is_ok() {
                    c.force_tools = env_flag("HARNESS_FORCE_TOOLS", c.force_tools);
                }
            }),
            Box::new(|c| {
                if std::env::var("HARNESS_APPROVE_TOOLS").is_ok() {
                    c.approve_tools = env_flag("HARNESS_APPROVE_TOOLS", c.approve_tools);
                }
            }),
            Box::new(|c| {
                if std::env::var("HARNESS_MULTI_AGENT").is_ok() {
                    c.multi_agent = env_flag("HARNESS_MULTI_AGENT", c.multi_agent);
                }
            }),
            Box::new(|c| {
                if std::env::var("HARNESS_CODEGEN").is_ok() {
                    c.codegen = env_flag("HARNESS_CODEGEN", c.codegen);
                }
            }),
            // 指定模型名才覆盖（空串不清除已有值）。
            Box::new(|c| {
                if let Ok(v) = std::env::var("HARNESS_CODEGEN_MODEL")
                    && !v.trim().is_empty()
                {
                    c.codegen_model = Some(v);
                }
            }),
            Box::new(|c| {
                if let Some(v) = std::env::var("HARNESS_MAX_TOKENS")
                    .ok()
                    .and_then(|s| s.parse::<u64>().ok())
                {
                    c.max_tokens = v;
                }
            }),
            Box::new(|c| {
                if let Some(v) = std::env::var("HARNESS_HISTORY_MAX_ITEMS")
                    .ok()
                    .and_then(|s| s.parse::<usize>().ok())
                {
                    c.history_max_items = v;
                }
            }),
            // 指定插件目录才覆盖（空串不清除已有值）。
            Box::new(|c| {
                if let Ok(v) = std::env::var("HARNESS_CODEGEN_DIR")
                    && !v.trim().is_empty()
                {
                    c.codegen_plugin_dir = Some(PathBuf::from(v));
                }
            }),
            Box::new(|c| c.theme = env_or("HARNESS_THEME", &c.theme)),
        ];
        for set in table {
            set(self);
        }
    }
}
