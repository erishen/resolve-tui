//! CLI 单次任务入口 `run`：把 agent 事件渲染到 stdout（无 TUI 路径）。
//!
//! 从 `agent/mod.rs` 抽出，使「对话状态」模块聚焦，CLI 编排逻辑独立成文件。

use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

use crate::agent::{AgentEvent, Approval};
use crate::{
    Config, HarnessError,
    agent::Conversation,
    mcp::McpManager,
    sandbox::{self, WORKSPACE_RETENTION_SECS},
    skills,
};

/// 无 TUI 的普通 CLI 路径：把事件渲染到 stdout。
///
/// 审批模式在 CLI 下无交互通道，故强制关闭并以哑通道喂入审批请求。
pub async fn run(task: &str, config: &Config) -> Result<String, HarnessError> {
    let (tx, mut rx) = mpsc::unbounded_channel::<AgentEvent>();
    let printer = tokio::spawn(async move {
        use std::io::Write;
        while let Some(ev) = rx.recv().await {
            match ev {
                AgentEvent::Token(t) => {
                    print!("{t}");
                    let _ = std::io::stdout().flush();
                }
                AgentEvent::ToolCall { name, id } => println!("\n[agent] -> {name} ({id})"),
                AgentEvent::ToolResult {
                    ok, chars, preview, ..
                } => match &preview {
                    Some(p) => println!(
                        "[agent] <- {} ({} chars)：{p}",
                        if ok { "ok" } else { "err" },
                        chars
                    ),
                    None => println!(
                        "[agent] <- {} ({} chars)",
                        if ok { "ok" } else { "err" },
                        chars
                    ),
                },
                AgentEvent::Reasoning(r) => println!("\n[推理] {r}"),
                AgentEvent::Usage {
                    input_tokens,
                    output_tokens,
                    had_tools,
                    total_tokens,
                    max_tokens,
                } => {
                    let budget = if max_tokens > 0 {
                        format!("{total_tokens}/{max_tokens}")
                    } else {
                        format!("{total_tokens}/∞")
                    };
                    println!(
                        "[agent] 用量 in={input_tokens} out={output_tokens} tools={} budget={budget}",
                        if had_tools { "Y" } else { "N" }
                    );
                }
                AgentEvent::ToolApproval { name, args, .. } => {
                    println!("\n[待确认] {name}: {args}")
                }
                AgentEvent::System(s) => println!("\n[系统] {s}"),
                AgentEvent::Error(m) => eprintln!("\n[agent] error: {m}"),
                AgentEvent::Iteration(_)
                | AgentEvent::Finished
                | AgentEvent::ToggleReasoning
                | AgentEvent::ClearScreen
                | AgentEvent::Export(_)
                | AgentEvent::Capabilities { .. }
                | AgentEvent::Document(_)
                | AgentEvent::Resumed(_)
                | AgentEvent::Quit => {}
            }
        }
    });

    let cli_config = Config {
        approve_tools: false,
        ..(*config).clone()
    };
    // 默认在项目下创建沙箱根目录（每任务工作区见 `[sandbox] 工作区` 提示）。
    if cli_config.policy.enabled {
        match sandbox::ensure_sandbox_root(&cli_config.sandbox_dir) {
            Ok(()) => {
                println!("[sandbox] 根目录: {}", cli_config.sandbox_dir.display());
                let removed = sandbox::prune_task_workspaces(
                    &cli_config.sandbox_dir,
                    WORKSPACE_RETENTION_SECS,
                );
                if removed > 0 {
                    println!("[sandbox] 已清理 {removed} 个过期任务工作区");
                }
            }
            Err(e) => println!("[sandbox] 根目录创建失败: {e}"),
        }
    }
    let (_atx, mut approval_rx) = mpsc::unbounded_channel::<Approval>();

    let mut conversation = Conversation::new();
    conversation.set_model(Arc::new(Mutex::new(config.model.clone())));
    // 技能与 MCP：启动时一次性加载/连接，失败不阻断。
    let (skills, skill_warnings) = skills::load_skills(&skills::skills_dir());
    conversation.set_skills(skills);
    for w in &skill_warnings {
        eprintln!("{w}");
    }
    let mgr = McpManager::connect_all(&config.mcp_servers).await;
    if !mgr.is_empty() {
        for line in mgr.status_lines() {
            println!("[mcp] {line}");
        }
        conversation.set_mcp(mgr);
    }
    let result = if cli_config.multi_agent {
        crate::agent::submit_roles(&mut conversation, task, &cli_config, &tx, &mut approval_rx)
            .await
    } else {
        conversation
            .submit(task, &cli_config, &tx, &mut approval_rx)
            .await
    };
    let _ = tx.send(AgentEvent::Finished);
    drop(tx);
    let _ = printer.await;
    result
}
