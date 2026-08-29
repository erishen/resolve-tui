use std::collections::HashSet;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;

use crate::{
    Config, HarnessError,
    mcp::McpManager,
    model::{InputItem, ResponseTool},
    sandbox::SandboxPolicy,
    skills::Skill,
    tools::builtin_tools,
};

// 主循环与纯函数辅助拆到子模块，保持本文件聚焦于「对话状态」与「提交入口」。
mod drive;
mod event;
mod helpers;
mod roles;
// 提交入口（submit）与 CLI 单次入口（run）拆到独立子模块，保持本文件聚焦对话状态。
mod cli;
mod submit;

// `submit`（本文件）直接按名称访问的辅助函数；其余两个仅测试模块 `agent_tests` 用到，
// 故用 `#[cfg(test)]` 限定，避免非测试编译下产生未使用导入告警。
pub(crate) use helpers::build_extra_instructions;
#[cfg(test)]
pub(crate) use helpers::{flatten_error, windowed_history};
// 多 Agent 三角色编排入口（与 `Conversation::submit` 同签名）。
pub(crate) use roles::submit_roles;
// CLI 单次任务入口（抽出到 cli.rs，保持本文件聚焦对话状态）。
pub use cli::run;

/// 以 0600 权限写入文本（unix 会话/记忆等敏感文件）；其余平台回退普通写入。
#[cfg(unix)]
pub(crate) fn write_private(path: &str, data: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};

    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(data.as_bytes())?;
    drop(f);
    // 显式收紧权限：mode(0o600) 仅对新文件生效，已存在的 0644 文件需覆盖权限。
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
pub(crate) fn write_private(path: &str, data: &str) -> std::io::Result<()> {
    std::fs::write(path, data)
}

/// 为一次任务创建独立沙箱工作区并套用到会话（沙箱启用时生效）。
/// 返回是否成功设置了工作区（false = 沙箱关闭或创建失败，回退无隔离）。
pub(crate) fn init_task_workspace(
    conversation: &mut Conversation,
    config: &Config,
    tx: &mpsc::UnboundedSender<AgentEvent>,
) -> bool {
    conversation.task_policy = None;
    if !config.policy.enabled {
        return false;
    }
    match crate::sandbox::new_task_workspace(&config.sandbox_dir) {
        Ok(ws) => {
            conversation.task_policy = Some(SandboxPolicy {
                enabled: true,
                allow_network: config.policy.allow_network,
                writable_roots: vec![ws.clone()],
                cwd: Some(ws.clone()),
            });
            let _ = tx.send(AgentEvent::System(format!(
                "[sandbox] 工作区: {}",
                ws.display()
            )));
            true
        }
        Err(e) => {
            let _ = tx.send(AgentEvent::System(format!(
                "[sandbox] 工作区创建失败，回退: {e}"
            )));
            false
        }
    }
}

// 事件与审批应答类型拆到 `event` 子模块；对外 API 仍从这里透出。
pub use event::{AgentEvent, Approval};

/// 多轮对话状态。
#[derive(Default)]
pub struct Conversation {
    /// 无状态模式下的完整本地历史（`input` 项）。
    input: Vec<InputItem>,
    /// 有状态模式（previous_response_id）下，上一轮响应的 id。
    previous_id: Option<String>,
    /// 是否使用有状态续接（见 `Config::stateful`）。
    stateful: bool,
    /// 累计消耗的 token 数（跨多轮累加，用于预算控制）。
    total_tokens: u64,
    /// 中途取消信号：用户按 Esc 中止本轮生成时置位，驱动循环与流式读取会检查它。
    cancel: Arc<AtomicBool>,
    /// 当前使用的模型名（运行时可被 `/model` 切换，请求发起时读取）。
    model: Arc<Mutex<String>>,
    /// MCP 远端工具（暴露名已合并进 LLM 工具列表）。
    extra_tools: Vec<ResponseTool>,
    /// MCP 管理器：负责把模型对远端工具的调用路由到对应 server。
    /// RwLock 支持运行时 `/mcp add|remove` 增删；call 走读锁，add/remove 走写锁。
    mcp: Option<Arc<tokio::sync::RwLock<McpManager>>>,
    /// MCP 连接状态行（`/mcp` 展示用，set_mcp 时快照）。
    mcp_status: Vec<String>,
    /// 已加载的技能（提示词包），按触发词命中注入 system prompt。
    skills: Vec<Skill>,
    /// 运行时被禁用的工具名（UI `/tools off` 维护）；提交请求前从工具列表剔除。
    disabled_tools: HashSet<String>,
    /// 当前任务的沙箱策略（每任务独立工作区）；`None` 表示用 `Config::policy`（无隔离）。
    task_policy: Option<SandboxPolicy>,
    /// codegen 学习的执行方式：TUI（常驻进程）置 true，答案返回后 spawn 后台任务，
    /// 不阻塞下一轮输入；CLI（单次进程）保持 false，退出前同步完成以保证插件落盘。
    #[cfg(feature = "codegen")]
    codegen_background: bool,
}

impl Conversation {
    pub fn new() -> Self {
        Self {
            cancel: Arc::new(AtomicBool::new(false)),
            model: Arc::new(Mutex::new(String::new())),
            ..Default::default()
        }
    }

    /// 带外部取消信号的构造（TUI 把信号句柄同时交给 UI 与 agent 任务）。
    pub fn with_cancel(cancel: Arc<AtomicBool>) -> Self {
        Self {
            cancel,
            ..Default::default()
        }
    }

    /// 注入运行时可切换的模型名（TUI / CLI 启动时把配置里的模型塞进来）。
    pub fn set_model(&mut self, model: Arc<Mutex<String>>) {
        self.model = model;
    }

    /// 注入 MCP 管理器；远端工具同时并入每轮请求的工具列表。
    pub fn set_mcp(&mut self, mgr: McpManager) {
        self.mcp_status = mgr.status_lines();
        self.extra_tools = mgr.llm_tools().to_vec();
        self.mcp = Some(Arc::new(tokio::sync::RwLock::new(mgr)));
    }

    /// `/mcp` 展示用：各 server 连接状态。
    pub fn mcp_status(&self) -> &[String] {
        &self.mcp_status
    }

    /// 合并后的工具列表（内置 + MCP），附启用状态；`/tools` 展示与测试用。
    pub fn visible_tools(&self) -> Vec<(String, String, bool)> {
        let mut all: Vec<(String, String)> = builtin_tools()
            .into_iter()
            .map(|t| (t.name, t.description))
            .collect();
        all.extend(
            self.extra_tools
                .iter()
                .map(|t| (t.name.clone(), t.description.clone())),
        );
        all.into_iter()
            .map(|(name, desc)| {
                let enabled = !self.disabled_tools.contains(&name);
                (name, desc, enabled)
            })
            .collect()
    }

    /// 启用 / 禁用某个工具；返回该工具名是否真实存在（未知名给 UI 报错提示）。
    pub fn set_tool_enabled(&mut self, name: &str, enabled: bool) -> bool {
        let known = self.visible_tools().iter().any(|(n, _, _)| n == name);
        if known {
            if enabled {
                self.disabled_tools.remove(name);
            } else {
                self.disabled_tools.insert(name.to_string());
            }
        }
        known
    }

    /// 运行时重建 MCP 连接（`/mcp reload`）；旧子进程随旧 Manager 一起回收。
    pub async fn reconnect_mcp(&mut self, config: &Config) {
        let mgr = McpManager::connect_all(&config.mcp_servers).await;
        self.mcp_status = mgr.status_lines();
        // 远端工具列表整体替换；已禁用的内置工具不受影响。
        self.extra_tools = mgr.llm_tools().to_vec();
        self.mcp = Some(Arc::new(tokio::sync::RwLock::new(mgr)));
    }

    /// 运行时挂载一个 MCP server（`/mcp add`）：握手成功后其工具立即并入下一轮请求。
    /// 返回新增加的工具数；失败时返回错误信息（不影响已有 server）。
    pub async fn add_mcp(
        &mut self,
        name: &str,
        command: &str,
        args: &[String],
    ) -> Result<usize, HarnessError> {
        let cfg = crate::mcp::McpServerConfig {
            name: name.to_string(),
            command: command.to_string(),
            args: args.to_vec(),
            env: Default::default(),
            call_timeout: Default::default(),
        };
        // 尚无 manager（配置为空）时现场建一个，保证首次 add 也能工作。
        let mgr = match &self.mcp {
            Some(m) => m.clone(),
            None => {
                let m = Arc::new(tokio::sync::RwLock::new(McpManager::new()));
                self.mcp = Some(m.clone());
                m
            }
        };
        let added = {
            let mut g = mgr.write().await;
            g.attach(&cfg).await?;
            // attach 成功后从 tools 里数出本 server 新增的暴露名。
            let prefix = format!("mcp_{}", name.replace(['-', '.'], "_"));
            g.llm_tools()
                .iter()
                .filter(|t| t.name.starts_with(&prefix))
                .count()
        };
        self.extra_tools = mgr.read().await.llm_tools().to_vec();
        self.mcp_status = mgr.read().await.status_lines();
        Ok(added)
    }

    /// 运行时摘除一个 MCP server（`/mcp remove`）：杀子进程并清掉其工具。
    pub async fn remove_mcp(&mut self, name: &str) -> Result<(), HarnessError> {
        let Some(m) = &self.mcp else {
            return Err(HarnessError::other("当前没有连接任何 MCP server"));
        };
        let mut g = m.write().await;
        if !g.detach(name) {
            return Err(HarnessError::other(format!(
                "未找到已连接的 server：{name}"
            )));
        }
        let prefix = format!("mcp_{}", name.replace(['-', '.'], "_"));
        self.extra_tools.retain(|t| !t.name.starts_with(&prefix));
        self.mcp_status = g.status_lines();
        Ok(())
    }

    /// 注入已加载的技能列表（启动时一次性加载）。
    pub fn set_skills(&mut self, skills: Vec<Skill>) {
        self.skills = skills;
    }

    /// 切换 codegen 学习是否后台执行（TUI 后台 / CLI 同步落盘）。
    #[cfg(feature = "codegen")]
    pub fn set_codegen_background(&mut self, yes: bool) {
        self.codegen_background = yes;
    }

    /// `/skills` 展示用：只读技能列表。
    pub fn skills(&self) -> &[Skill] {
        &self.skills
    }
}

impl Conversation {
    /// 把当前对话历史与累计用量持久化为 JSON。
    ///
    /// 会话含账号能看到的完整对话（可能含粘贴的密钥/敏感信息），文件以 0600
    /// 权限写入，避免同机其它用户可读。
    pub fn save(&self, path: &str) -> Result<(), HarnessError> {
        let payload = serde_json::json!({
            "version": 1u32,
            "total_tokens": self.total_tokens,
            "messages": self.input,
        });
        let data = serde_json::to_string_pretty(&payload)
            .map_err(|e| HarnessError::tool(format!("序列化会话失败: {e}")))?;
        write_private(path, &data)
            .map_err(|e| HarnessError::tool(format!("写入 {path} 失败: {e}")))?;
        Ok(())
    }

    /// 从 JSON 恢复对话历史与累计用量；兼容旧版裸数组格式（其 `total_tokens` 视为 0）。
    /// 恢复后清空有状态游标（无法续接 `previous_response_id`），
    /// 但保留 `total_tokens`，使预算控制在续接后能从「已消耗」处继续计数，
    /// 而不是从 0 重新计数导致实际累计超出 `HARNESS_MAX_TOKENS`。
    pub fn load(&mut self, path: &str) -> Result<(), HarnessError> {
        let data = std::fs::read_to_string(path)
            .map_err(|e| HarnessError::tool(format!("读取 {path} 失败: {e}")))?;
        let value: serde_json::Value = serde_json::from_str(&data)
            .map_err(|e| HarnessError::tool(format!("解析会话失败: {e}")))?;

        let (items, total): (Vec<InputItem>, u64) = match &value {
            serde_json::Value::Array(_) => {
                let items: Vec<InputItem> = serde_json::from_value(value)
                    .map_err(|e| HarnessError::tool(format!("解析会话失败: {e}")))?;
                (items, 0)
            }
            serde_json::Value::Object(_) => {
                let items: Vec<InputItem> = serde_json::from_value(
                    value
                        .get("messages")
                        .cloned()
                        .unwrap_or(serde_json::Value::Array(vec![])),
                )
                .map_err(|e| HarnessError::tool(format!("解析会话失败: {e}")))?;
                let total = value
                    .get("total_tokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                (items, total)
            }
            _ => return Err(HarnessError::tool(format!("会话格式无法识别: {path}"))),
        };

        self.input = items;
        self.previous_id = None;
        self.total_tokens = total;
        Ok(())
    }

    /// 只读访问累计 token 用量（预算审计 / 测试用）。
    pub fn total_tokens(&self) -> u64 {
        self.total_tokens
    }

    /// 清空对话历史与累计用量（对应 `/clear`）。
    pub fn clear(&mut self) {
        self.input.clear();
        self.previous_id = None;
        self.total_tokens = 0;
    }

    /// 只读访问当前对话历史（测试与调试用）。
    pub fn input(&self) -> &[InputItem] {
        &self.input
    }

    /// 可变访问对话历史（测试构造场景用）。
    pub fn input_mut(&mut self) -> &mut Vec<InputItem> {
        &mut self.input
    }
}

#[cfg(test)]
#[path = "../agent_tests.rs"]
mod tests;
