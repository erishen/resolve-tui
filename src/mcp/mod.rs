//! 极简 MCP（Model Context Protocol）stdio 客户端。
//!
//! 启动时按配置拉起各 server 子进程，走换行分隔的 JSON-RPC 2.0：
//! `initialize` 握手 → `notifications/initialized` → `tools/list` 拉取工具，
//! 之后把远端工具以 `mcp_<server>_<tool>` 的暴露名合并进 LLM 工具列表；
//! 模型调用时按暴露名路由回对应 server 的 `tools/call`。
//!
//! 失败隔离：单个 server 连不上只记录状态、跳过，不阻断整体启动。

use std::collections::HashMap;
use std::time::Duration;

use crate::model::ResponseTool;

/// 单个 MCP server 的启动配置（来自 TOML `[mcp_servers.<name>]`）。
#[derive(Clone, Debug)]
pub struct McpServerConfig {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
}

/// 握手 / 枚举的超时；工具调用可能较慢，给更长的窗口。
pub(crate) const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);
pub(crate) const CALL_TIMEOUT: Duration = Duration::from_secs(120);

/// 远端工具的中间表示（tools/list 的 result.tools[] 项）。
pub(crate) struct RemoteTool {
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) schema: serde_json::Value,
}

pub struct McpManager {
    clients: Vec<client::McpClient>,
    /// 远端合并进 LLM 的工具定义（暴露名已去重）。
    tools: Vec<ResponseTool>,
    /// 暴露名 → (server 名, 原始工具名)。按名而非下标路由，摘除单个 server 不影响其余。
    routing: HashMap<String, (String, String)>,
    /// 每个 server 的连接结果（供 `/mcp` 展示）。
    status: Vec<String>,
}

mod client;
mod manager;
mod protocol;

#[cfg(test)]
mod tests {
    use super::protocol::{exposed_name, extract_text, parse_tools};
    use super::*;

    #[test]
    fn exposed_name_sanifies_and_truncates() {
        assert_eq!(
            exposed_name("my server", "read-file"),
            "mcp_my_server_read-file"
        );
        // 中文 server 名（3 字符）→ 3 个下划线；加上分隔符共 5 个，前缀 8 字符。
        let long = exposed_name("服务器", &"t".repeat(100));
        assert_eq!(long.chars().count(), 64);
        assert_eq!(long, format!("mcp_____{}", "t".repeat(56)));
        assert!(long.chars().count() <= 64);
        assert!(
            long.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        );
    }

    #[test]
    fn parse_tools_defaults_missing_schema() {
        let listing = serde_json::json!({"tools": [
            {"name": "a", "description": "d"},
            {"name": "b", "inputSchema": {"type": "object", "properties": {}}}
        ]});
        let tools = parse_tools(&listing).unwrap();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].schema, serde_json::json!({"type": "object"}));
        assert_eq!(tools[1].description, "");
    }

    #[test]
    fn extract_text_joins_blocks_and_falls_back() {
        let r = serde_json::json!({"content": [
            {"type": "text", "text": "行1"},
            {"type": "image", "data": "..."},
            {"type": "text", "text": "行2"}
        ]});
        assert_eq!(extract_text(&r), "行1\n行2");
        assert_eq!(
            extract_text(&serde_json::json!({})),
            "（工具响应缺少 content）"
        );
    }

    /// 用 sh 脚本伪造一个会说 JSON-RPC 的 MCP server，验证完整链路：
    /// 握手 → tools/list → tools/call 路由与文本提取。
    /// sed 从请求行里回提 id，因此不依赖客户端的 id 分配顺序。
    /// `$1` 为对外暴露的工具名，便于测试多 server 场景（默认 echo）。
    const FAKE_SERVER: &str = r#"#!/bin/sh
TOOL="${1:-echo}"
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"fake","version":"1"}}}\n' "$id"
      ;;
    *'"method":"tools/list"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"%s","description":"Echo tool","inputSchema":{"type":"object","properties":{"text":{"type":"string"}}}}]}}\n' "$id" "$TOOL"
      ;;
    *'"method":"tools/call"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"echo-ok"},{"type":"other"}]}}\n' "$id"
      ;;
    *)
      ;;
  esac
done
"#;

    fn write_fake_server(tool: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        // 每次调用用独立目录：并行测试互不删对方的脚本（否则 server 进程会读到被删文件）。
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("harness_mcp_test_{}_{}", std::process::id(), n));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join(format!("fake-server-{tool}.sh"));
        std::fs::write(&script, FAKE_SERVER).unwrap();
        script
    }

    fn fake_cfg(name: &str, tool: &str) -> McpServerConfig {
        McpServerConfig {
            name: name.to_string(),
            command: "sh".to_string(),
            args: vec![
                write_fake_server(tool).to_string_lossy().to_string(),
                tool.to_string(),
            ],
            env: HashMap::new(),
        }
    }

    #[tokio::test]
    async fn connects_lists_and_routes_calls() {
        let mgr = McpManager::connect_all(&[fake_cfg("fake", "echo")]).await;
        let status = mgr.status_lines();
        assert_eq!(status.len(), 1);
        assert!(status[0].contains("已连接"), "应连接成功: {status:?}");
        assert!(status[0].contains("1 个工具"));

        let tools = mgr.llm_tools();
        assert_eq!(tools.len(), 1);
        assert_eq!(
            tools[0].name, "mcp_fake_echo",
            "暴露名应为 mcp_<server>_<tool>"
        );
        assert_eq!(tools[0].description, "Echo tool");
        assert!(mgr.routes("mcp_fake_echo"));
        assert!(!mgr.routes("shell"));

        let out = mgr
            .call("mcp_fake_echo", "{\"text\":\"hi\"}")
            .await
            .unwrap();
        assert_eq!(out, "echo-ok", "应只拼接 text 块并路由回原工具名");

        let err = mgr.call("mcp_fake_unknown", "{}").await.unwrap_err();
        assert!(err.to_string().contains("未知 MCP 工具"));
    }

    #[tokio::test]
    async fn broken_server_is_skipped_without_blocking_others() {
        let mut bad = fake_cfg("bad", "x");
        bad.command = "/nonexistent-mcp-binary-xyz".to_string();
        let mgr = McpManager::connect_all(&[bad, fake_cfg("good", "echo")]).await;
        let status = mgr.status_lines();
        assert_eq!(status.len(), 2);
        assert!(
            status[0].contains("连接失败"),
            "坏 server 应被跳过: {status:?}"
        );
        assert!(
            status[1].contains("已连接"),
            "好 server 不受影响: {status:?}"
        );
        assert_eq!(mgr.llm_tools().len(), 1);
        assert!(mgr.call("mcp_good_echo", "{}").await.is_ok());
    }

    // 运行时增量挂载 / 摘除：attach 注册路由；detach 只清自己的工具，不影响其余 server。
    #[tokio::test]
    async fn attach_then_detach_isolates_servers() {
        let mut mgr = McpManager::new();
        assert!(mgr.is_empty());

        mgr.attach(&fake_cfg("alpha", "tool_a"))
            .await
            .expect("alpha 挂载");
        mgr.attach(&fake_cfg("beta", "tool_b"))
            .await
            .expect("beta 挂载");
        assert_eq!(mgr.llm_tools().len(), 2);
        assert!(mgr.routes("mcp_alpha_tool_a"));
        assert!(mgr.routes("mcp_beta_tool_b"));
        assert!(mgr.has_server("alpha") && mgr.has_server("beta"));

        // 同名重复挂载应报错而非产生半成品。
        assert!(mgr.attach(&fake_cfg("alpha", "dup")).await.is_err());
        assert_eq!(mgr.llm_tools().len(), 2);

        // 摘除 alpha：其工具消失，beta 的路由与调用不受影响。
        assert!(mgr.detach("alpha"));
        assert!(!mgr.has_server("alpha"));
        assert!(!mgr.routes("mcp_alpha_tool_a"));
        assert!(mgr.routes("mcp_beta_tool_b"));
        assert_eq!(mgr.llm_tools().len(), 1);
        assert_eq!(mgr.call("mcp_beta_tool_b", "{}").await.unwrap(), "echo-ok");

        // 摘除不存在的名字返回 false。
        assert!(!mgr.detach("nope"));

        // 摘除最后一个后回到空态，可重新挂载。
        assert!(mgr.detach("beta"));
        assert!(mgr.is_empty());
        mgr.attach(&fake_cfg("gamma", "tool_c")).await.unwrap();
        assert!(mgr.routes("mcp_gamma_tool_c"));
    }
}
