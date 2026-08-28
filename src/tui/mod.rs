//! 终端交互界面（TUI）。
//!
//! 模块划分：
//! - [`app`]：界面状态（滚动历史 / 输入框 / 审批）与 agent 事件处理
//! - [`input`]：键盘事件（提交/编辑/翻页/审批应答）与补全
//! - [`commands`]：会话控制命令（/list /create /apply …）
//! - [`render`]：布局渲染与折行
//! - [`theme`]：配色主题与终端背景探测
//! - [`util`]：剪贴板 / 路径显示 / 参数美化

mod app;
mod commands;
mod format;
mod input;
mod render;
mod theme;
mod util;
mod wrap;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crossterm::event::{DisableBracketedPaste, EnableBracketedPaste, EventStream};
use futures::StreamExt;
use ratatui::{Terminal, backend::CrosstermBackend};
use tokio::sync::mpsc;
use tokio::time::interval;

use crate::{
    Config, HarnessError,
    agent::{AgentEvent, Approval, Conversation},
    sessions::{resolve, sessions_dir},
};

use app::App;
use commands::handle_control;
use input::handle_key;
use render::{push_help, ui};
use theme::{Role, Theme, detect_is_light_bg};
use util::display_path;

/// 启动终端交互界面。任务从底部输入框提交（Enter），多轮连续对话。
/// `resume` 为启动时自动加载的会话（索引 / 名称 / 显式路径均可）。
/// `startup_notes` 为进入 TUI 前收集到的诊断（配置告警 / .env 权限等），
/// 作为系统消息呈现在聊天区，而非直接打到 stderr 在备用屏外闪现。
pub async fn run_tui(
    config: std::sync::Arc<Config>,
    resume: Option<String>,
    startup_notes: Vec<String>,
) -> std::io::Result<()> {
    crossterm::terminal::enable_raw_mode().ok();
    let mut stdout = std::io::stdout();
    crossterm::execute!(
        stdout,
        crossterm::terminal::EnterAlternateScreen,
        EnableBracketedPaste
    )
    .ok();
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let (tx, mut rx) = mpsc::unbounded_channel::<AgentEvent>();
    let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<String>();
    let (approval_tx, mut approval_rx) = mpsc::unbounded_channel::<Approval>();

    // 项目下默认创建沙箱根目录（每任务工作区见 `[sandbox] 工作区` 提示），并顺带清理过期工作区。
    if config.policy.enabled && crate::sandbox::ensure_sandbox_root(&config.sandbox_dir).is_ok() {
        crate::sandbox::prune_task_workspaces(
            &config.sandbox_dir,
            crate::sandbox::WORKSPACE_RETENTION_SECS,
        );
    }

    // 未显式指定 --resume 时，若存在上次自动存档（last）则自动续接，实现「退出即保存、启动即恢复」。
    let resume = resume.or_else(|| {
        let last = sessions_dir().join("last.json");
        if last.exists() {
            Some("last".to_string())
        } else {
            None
        }
    });

    // 取消信号在 UI 与 agent 任务间共享：运行中按 Esc 置位，驱动循环会中止本轮。
    let cancel = Arc::new(AtomicBool::new(false));
    // 多 Agent（PSE）模式开关在 UI 与 agent 任务间共享，支持 `/pse` 运行时切换。
    let pse = Arc::new(AtomicBool::new(config.multi_agent));
    // 模型名在 UI 与 agent 任务间共享，支持 `/model` 运行时切换。
    let model = Arc::new(Mutex::new(config.model.clone()));

    let mut app = App {
        cancel: cancel.clone(),
        model: model.clone(),
        status: format!(
            "sandbox={} approve={}",
            if config.policy.enabled { "on" } else { "off" },
            if config.approve_tools { "on" } else { "off" }
        ),
        ..Default::default()
    };
    // 选择配色主题：`auto` 探测终端背景色（OSC 11），否则按配置用亮/暗主题。
    app.theme = match config.theme.to_ascii_lowercase().as_str() {
        "light" | "white" | "lightbg" => Theme::light(),
        "auto" => {
            if detect_is_light_bg().await {
                Theme::light()
            } else {
                Theme::dark()
            }
        }
        _ => Theme::dark(),
    };
    push_help(&mut app);
    if let Some(key) = &resume {
        app.push(
            Role::System,
            format!("启动时载入会话：{key}（结果见下方提示）"),
        );
    }

    // agent 任务：独享 conversation 与审批接收端；以 / 开头的行视为控制命令。
    let agent_tx = tx.clone();
    let agent = tokio::spawn({
        let config = config.clone();
        let pse = pse.clone();
        async move {
            let mut conversation = Conversation::with_cancel(cancel.clone());
            conversation.set_model(model.clone());
            // 常驻进程：codegen 事后学习放后台执行，答案返回即交还输入焦点。
            #[cfg(feature = "codegen")]
            conversation.set_codegen_background(true);
            // 技能与 MCP：启动时一次性加载/连接（失败不阻断）。
            let (skills, skill_warnings) = crate::skills::load_skills(&crate::skills::skills_dir());
            conversation.set_skills(skills);
            // 进入 TUI 前收集到的诊断（配置/.env）+ 技能加载告警，统一作为系统消息呈现，
            // 避免直接打 stderr 污染备用屏、退出时又被 LeaveAlternateScreen 闪出来。
            for note in startup_notes.iter().chain(skill_warnings.iter()) {
                let _ = agent_tx.send(AgentEvent::System(note.clone()));
            }
            let mut mgr = crate::mcp::McpManager::connect_all(&config.mcp_servers).await;
            let mcp_status = mgr.status_lines();
            if !mgr.is_empty() {
                conversation.set_mcp(std::mem::take(&mut mgr));
            }
            // 能力面：标题栏常驻计数（Capabilities）+ 历史里一段对齐的明细日志。
            let _ = agent_tx.send(AgentEvent::Capabilities {
                skills: conversation.skills().len(),
                tools: conversation.visible_tools().len(),
                mcp_online: mcp_status.iter().filter(|l| l.contains("已连接")).count(),
            });
            for line in crate::tui::format::capability_lines(
                &conversation.visible_tools(),
                conversation.skills(),
                &mcp_status,
            ) {
                let _ = agent_tx.send(AgentEvent::System(line));
            }
            // 启动时自动载入会话（若有）：支持序号 / 名称 / 显式路径。
            if let Some(key) = &resume {
                let dir = sessions_dir();
                let path = resolve(&dir, key)
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|| key.clone());
                match conversation.load(&path) {
                    Ok(_) => {
                        // 空会话（如首次自动存档）不提示，避免无意义刷屏。
                        if !conversation.input().is_empty() {
                            let _ = agent_tx.send(AgentEvent::System(format!(
                                "已载入会话 {}",
                                display_path(&path)
                            )));
                        }
                        // 把恢复的历史回放到可见记录，否则续接后屏幕空白。
                        let _ = agent_tx.send(AgentEvent::Resumed(conversation.input().to_vec()));
                    }
                    Err(e) => {
                        let _ = agent_tx.send(AgentEvent::Error(e.to_string()));
                    }
                }
            }
            while let Some(line) = cmd_rx.recv().await {
                if line.starts_with('/') || commands::is_quit_command(&line) {
                    handle_control(&mut conversation, &agent_tx, &line, &model, &config, &pse)
                        .await;
                    continue;
                }
                let submit_result = if pse.load(Ordering::SeqCst) {
                    crate::agent::submit_roles(
                        &mut conversation,
                        &line,
                        &config,
                        &agent_tx,
                        &mut approval_rx,
                    )
                    .await
                } else {
                    conversation
                        .submit(&line, &config, &agent_tx, &mut approval_rx)
                        .await
                };
                if let Err(e) = submit_result {
                    // 用户主动取消不是错误，用中性提示而非红色报错。
                    if matches!(e, HarnessError::Cancelled) {
                        let _ = agent_tx.send(AgentEvent::System("— 已取消 —".to_string()));
                    } else {
                        let _ = agent_tx.send(AgentEvent::Error(e.to_string()));
                    }
                }
                let _ = agent_tx.send(AgentEvent::Finished);
            }
            // 命令通道关闭（UI 退出）→ 自动存档到 last，下次启动可续接。
            let last = sessions_dir().join("last.json");
            let _ = conversation.save(&last.to_string_lossy());
        }
    });

    // 看门狗：agent 任务一旦 panic 崩溃，向 UI 报错并复位「运行中」状态，
    // 否则输入框会永远卡在运行态（Esc 被 running 拦住），只能 Ctrl-C 退出。
    let watchdog_tx = tx.clone();
    tokio::spawn(async move {
        if let Err(e) = agent.await {
            let _ = watchdog_tx.send(AgentEvent::Error(format!("agent 任务异常退出：{e}")));
            let _ = watchdog_tx.send(AgentEvent::Finished);
        }
    });

    let mut reader = EventStream::new();
    let mut tick = interval(Duration::from_millis(80));
    loop {
        tokio::select! {
            maybe = reader.next() => {
                if let Some(Ok(ev)) = maybe {
                    handle_key(ev, &mut app, &cmd_tx, &approval_tx);
                }
            }
            Some(ev) = rx.recv() => {
                app.on_event(ev);
            }
            _ = tick.tick() => {
                app.ticks = app.ticks.wrapping_add(1);
            }
        }

        terminal.draw(|f| ui(f, &mut app))?;
        if app.should_quit {
            break;
        }
    }

    // agent 句柄已交给看门狗 await；退出后残留任务由 runtime 关闭时回收。
    crossterm::terminal::disable_raw_mode().ok();
    crossterm::execute!(
        std::io::stdout(),
        crossterm::terminal::LeaveAlternateScreen,
        DisableBracketedPaste
    )
    .ok();
    Ok(())
}
