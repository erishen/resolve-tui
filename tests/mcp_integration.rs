//! 集成测试：通过公开 API（`resolve_tui::mcp`）验证 MCP 客户端完整链路。
//! 单元测试只能触达 `pub(crate)` 项；此处专门守住对外合同（server 连接 / 路由 / 调用）。

use resolve_tui::mcp::{McpManager, McpServerConfig};
use std::collections::HashMap;

/// 用 sh + sed 伪造一个会说 JSON-RPC 的 MCP server（sed 从请求行回提 id，不依赖客户端分配顺序）。
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
  esac
done
"#;

fn fake_cfg(dir: &std::path::Path, name: &str, tool: &str) -> McpServerConfig {
    let script = dir.join(format!("fake-{tool}.sh"));
    std::fs::write(&script, FAKE_SERVER).unwrap();
    McpServerConfig {
        name: name.to_string(),
        command: "sh".to_string(),
        args: vec![script.to_string_lossy().to_string(), tool.to_string()],
        env: HashMap::new(),
        call_timeout: std::time::Duration::default(),
    }
}

#[tokio::test]
async fn public_api_connects_lists_routes_and_calls() {
    let dir = std::env::temp_dir().join(format!("harness_itest_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let mgr = McpManager::connect_all(&[fake_cfg(&dir, "fake", "echo")]).await;
    assert_eq!(mgr.status_lines().len(), 1);
    assert!(mgr.status_lines()[0].contains("已连接"));

    let tools = mgr.llm_tools();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "mcp_fake_echo");
    assert!(mgr.routes("mcp_fake_echo"));

    let out = mgr
        .call("mcp_fake_echo", "{\"text\":\"hi\"}")
        .await
        .unwrap();
    assert_eq!(out, "echo-ok");

    let _ = std::fs::remove_dir_all(&dir);
}
