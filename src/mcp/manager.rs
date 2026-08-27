//! `McpManager`：server 生命周期编排（连接 / 增量挂载 / 摘除 / 路由 / 调用转发）。

use std::collections::HashMap;

use crate::{HarnessError, model::ResponseTool};

use super::client::McpClient;
use super::protocol::{exposed_name, extract_text, sanitize_name, timeout};
use super::{CALL_TIMEOUT, McpServerConfig};

impl Default for super::McpManager {
    fn default() -> Self {
        Self::new()
    }
}

impl super::McpManager {
    /// 空 manager（无任何 server）；运行时 `/mcp add` 可增量挂载。
    pub fn new() -> Self {
        Self {
            clients: Vec::new(),
            tools: Vec::new(),
            routing: HashMap::new(),
            status: Vec::new(),
        }
    }

    /// 连接全部配置的 server；任何一个失败都不影响其它 server。
    pub async fn connect_all(configs: &[McpServerConfig]) -> Self {
        let mut mgr = Self::new();
        for cfg in configs {
            if let Err(e) = mgr.attach(cfg).await {
                eprintln!("[mcp] server {} 连接失败，已跳过: {e}", cfg.name);
                mgr.status.push(format!("{}：连接失败（{e}）", cfg.name));
            }
        }
        mgr
    }

    /// 增量挂载一个 server：握手、拉取工具并注册路由。
    /// 失败时不留半成品（client 被 drop 即杀掉子进程）。
    pub async fn attach(&mut self, cfg: &McpServerConfig) -> Result<(), HarnessError> {
        if self.has_server(&cfg.name) {
            return Err(HarnessError::tool(format!("server {} 已连接", cfg.name)));
        }
        let (client, remote_tools) = McpClient::start(cfg).await?;
        let n = remote_tools.len();
        let mut added = 0usize;
        for t in &remote_tools {
            let exposed = exposed_name(&cfg.name, &t.name);
            // 不同 server 的同名工具 / 非法字符：先到先得，其余丢弃并告警。
            if self.routing.contains_key(&exposed) {
                eprintln!("[mcp] 工具暴露名冲突，已忽略 {}.{}", cfg.name, t.name);
                continue;
            }
            self.routing
                .insert(exposed.clone(), (cfg.name.clone(), t.name.clone()));
            self.tools.push(ResponseTool {
                tool_type: "function".to_string(),
                name: exposed,
                description: t.description.clone(),
                parameters: t.schema.clone(),
            });
            added += 1;
        }
        self.status
            .push(format!("{}：已连接（{added}/{n} 个工具）", cfg.name));
        self.clients.push(client);
        Ok(())
    }

    /// 摘除一个 server：杀掉子进程、清理其全部工具与路由。返回是否确有摘除。
    pub fn detach(&mut self, name: &str) -> bool {
        let before = self.clients.len();
        self.clients.retain(|c| c.name != name);
        if self.clients.len() == before {
            return false;
        }
        self.routing.retain(|_, (srv, _)| srv != name);
        let prefix = format!("mcp_{}", sanitize_name(name));
        self.tools.retain(|t| !t.name.starts_with(&prefix));
        self.status.retain(|l| !l.starts_with(&format!("{name}：")));
        true
    }

    /// 是否已挂载该名字的 server。
    pub fn has_server(&self, name: &str) -> bool {
        self.clients.iter().any(|c| c.name == name)
    }

    /// 是否有可用的远端工具。
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// 合并进 LLM 请求的远端工具定义。
    pub fn llm_tools(&self) -> &[ResponseTool] {
        &self.tools
    }

    /// 该名字是否路由到某个 MCP server。
    pub fn routes(&self, exposed: &str) -> bool {
        self.routing.contains_key(exposed)
    }

    /// 转发一次 `tools/call`，把 text 内容块拼接为字符串返回。
    pub async fn call(&self, exposed: &str, arguments_json: &str) -> Result<String, HarnessError> {
        let Some((server, original)) = self.routing.get(exposed) else {
            return Err(HarnessError::tool(format!("未知 MCP 工具: {exposed}")));
        };
        let args: serde_json::Value =
            serde_json::from_str(arguments_json.trim()).unwrap_or(serde_json::json!({}));
        let client = self
            .clients
            .iter()
            .find(|c| &c.name == server)
            .ok_or_else(|| HarnessError::tool("MCP server 已断开"))?;
        let result = timeout(
            client.request(
                "tools/call",
                serde_json::json!({"name": original, "arguments": args}),
            ),
            CALL_TIMEOUT,
        )
        .await?;
        if result.get("isError").and_then(|v| v.as_bool()) == Some(true) {
            return Err(HarnessError::tool(format!(
                "工具 {original} 返回错误: {}",
                extract_text(&result)
            )));
        }
        Ok(extract_text(&result))
    }

    /// `/mcp` 展示用状态行。
    pub fn status_lines(&self) -> Vec<String> {
        self.status.clone()
    }
}
