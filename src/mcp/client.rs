//! `McpClient`：单个 server 的 stdio JSON-RPC 2.0 传输（握手 / 请求 / 通知）。

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::Mutex as AsyncMutex;

use crate::HarnessError;

use super::protocol::{parse_tools, timeout};
use super::{HANDSHAKE_TIMEOUT, McpServerConfig, RemoteTool};

pub(crate) struct McpClient {
    /// 所属 server 名（attach/detach 按名操作）。
    pub(crate) name: String,
    /// 单次 tools/call 超时（取自 server 配置，长任务可放大）。
    pub(crate) call_timeout: Duration,
    _child: Child,
    stdin: AsyncMutex<ChildStdin>,
    reader: AsyncMutex<BufReader<ChildStdout>>,
    next_id: AtomicU64,
}

impl McpClient {
    /// 启动子进程并完成握手 + 工具枚举，返回 (客户端, 远端工具)。
    pub(crate) async fn start(
        cfg: &McpServerConfig,
    ) -> Result<(Self, Vec<RemoteTool>), HarnessError> {
        let mut child = tokio::process::Command::new(&cfg.command)
            .args(&cfg.args)
            .envs(&cfg.env)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            // stderr 丢弃，避免 server 日志打乱 TUI 渲染。
            .stderr(std::process::Stdio::null())
            // Manager 被 drop 时连带杀掉子进程，防止孤儿进程常驻。
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| HarnessError::tool(format!("启动 {}: {e}", cfg.command)))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| HarnessError::tool("子进程 stdin 不可用"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| HarnessError::tool("子进程 stdout 不可用"))?;
        let client = Self {
            name: cfg.name.clone(),
            call_timeout: cfg.call_timeout(),
            _child: child,
            stdin: AsyncMutex::new(stdin),
            reader: AsyncMutex::new(BufReader::new(stdout)),
            next_id: AtomicU64::new(0),
        };

        let init_params = serde_json::json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {"name": "resolve-tui", "version": env!("CARGO_PKG_VERSION")},
        });
        timeout(client.request("initialize", init_params), HANDSHAKE_TIMEOUT).await?;
        client.notify("notifications/initialized").await?;

        let listing = timeout(
            client.request("tools/list", serde_json::json!({})),
            HANDSHAKE_TIMEOUT,
        )
        .await?;
        Ok((client, parse_tools(&listing)?))
    }

    /// 发送一次 JSON-RPC 请求并等待同 id 的响应；
    /// 中途收到的通知 / 日志行直接忽略。
    pub(crate) async fn request(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, HarnessError> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst) + 1;
        let line = format!(
            "{}\n",
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": method,
                "params": params,
            })
        );
        {
            let mut stdin = self.stdin.lock().await;
            stdin
                .write_all(line.as_bytes())
                .await
                .map_err(|e| HarnessError::tool(format!("写入 {method} 请求失败: {e}")))?;
            stdin.flush().await.ok();
        }

        loop {
            let mut buf = String::new();
            let n = self
                .reader
                .lock()
                .await
                .read_line(&mut buf)
                .await
                .map_err(|e| HarnessError::tool(format!("读取 {method} 响应失败: {e}")))?;
            if n == 0 {
                return Err(HarnessError::tool(format!(
                    "server 在等待 {method} 响应时关闭了连接"
                )));
            }
            let Ok(v) = serde_json::from_str::<serde_json::Value>(buf.trim()) else {
                continue; // 非 JSON 行（如调试输出）：忽略。
            };
            if v.get("id").and_then(|x| x.as_u64()) != Some(id) {
                continue;
            }
            if let Some(err) = v.get("error") {
                return Err(HarnessError::tool(format!("{method} 失败: {err}")));
            }
            return Ok(v.get("result").cloned().unwrap_or(serde_json::Value::Null));
        }
    }

    /// 发送通知（无 id、无响应）。
    async fn notify(&self, method: &str) -> Result<(), HarnessError> {
        let line = format!(
            "{}\n",
            serde_json::json!({"jsonrpc": "2.0", "method": method})
        );
        let mut stdin = self.stdin.lock().await;
        stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|e| HarnessError::tool(format!("写入 {method} 失败: {e}")))?;
        stdin.flush().await.ok();
        Ok(())
    }
}
