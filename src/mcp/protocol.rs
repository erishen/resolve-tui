//! 协议辅助：请求超时包装、tools/list 解析、暴露名生成、文本块提取。

use crate::HarnessError;

use super::RemoteTool;

/// 给请求包一层超时；超时视为该 server 不可用。
pub(crate) async fn timeout<F>(
    fut: F,
    d: std::time::Duration,
) -> Result<serde_json::Value, HarnessError>
where
    F: std::future::Future<Output = Result<serde_json::Value, HarnessError>>,
{
    match tokio::time::timeout(d, fut).await {
        Ok(r) => r,
        Err(_) => Err(HarnessError::tool("MCP 请求超时")),
    }
}

/// 解析 tools/list 结果；缺 inputSchema 的工具用空 object 兜底（OpenAI 要求 object）。
pub(crate) fn parse_tools(listing: &serde_json::Value) -> Result<Vec<RemoteTool>, HarnessError> {
    let Some(arr) = listing.get("tools").and_then(|t| t.as_array()) else {
        return Err(HarnessError::tool("tools/list 响应缺少 tools 数组"));
    };
    let mut out = Vec::new();
    for t in arr {
        let Some(name) = t.get("name").and_then(|n| n.as_str()) else {
            continue;
        };
        out.push(RemoteTool {
            name: name.to_string(),
            description: t
                .get("description")
                .and_then(|d| d.as_str())
                .unwrap_or_default()
                .to_string(),
            schema: t
                .get("inputSchema")
                .cloned()
                .unwrap_or(serde_json::json!({"type": "object"})),
        });
    }
    Ok(out)
}

/// 暴露名：OpenAI function name 仅允许 `[a-zA-Z0-9_-]` 且 ≤64 字符，
/// 统一映射为 `mcp_<server>_<tool>` 并替换非法字符。
pub(crate) fn exposed_name(server: &str, tool: &str) -> String {
    // 按字符（而非字节）截断，避免把多字节字符切成非法 UTF-8。
    format!("mcp_{}_{}", sanitize_name(server), sanitize_name(tool))
        .chars()
        .take(64)
        .collect()
}

/// 单段名字净化：非 `[a-zA-Z0-9_-]` 一律替换为下划线。
pub(crate) fn sanitize_name(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// 拼接 result.content[] 中的 text 块；无内容时给出可读占位。
pub(crate) fn extract_text(result: &serde_json::Value) -> String {
    match result.get("content").and_then(|c| c.as_array()) {
        Some(items) => {
            let texts: Vec<&str> = items
                .iter()
                .filter(|i| i.get("type").and_then(|t| t.as_str()) == Some("text"))
                .filter_map(|i| i.get("text").and_then(|t| t.as_str()))
                .collect();
            if texts.is_empty() {
                "（工具无文本输出）".to_string()
            } else {
                texts.join("\n")
            }
        }
        None => "（工具响应缺少 content）".to_string(),
    }
}
