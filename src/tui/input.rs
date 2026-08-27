//! 输入处理：键盘事件（提交/编辑/翻页/审批应答）与会话控制命令（/list /create …）。

use std::sync::atomic::Ordering;

use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};
use tokio::sync::mpsc;

use super::app::App;
use super::commands::is_quit_command;
use super::theme::Role;
use super::util::copy_to_clipboard;
use crate::agent::Approval;

/// 顶层命令表（补全用；与 handle_control 实际支持的保持一致）。
const COMMANDS: &[&str] = &[
    "/list",
    "/create",
    "/apply",
    "/save",
    "/load",
    "/clear",
    "/rm",
    "/model",
    "/reasoning",
    "/export",
    "/tools",
    "/skills",
    "/mcp",
    "/remember",
    "/examples",
    "/help",
    "/quit",
    "/exit",
    "/q",
];

/// 参数候选：按首个命令字分发；`sessions` 走磁盘列表由调用方填充。
fn arg_candidates(cmd: &str, sessions: &[String]) -> Vec<String> {
    match cmd {
        "/mcp" => vec!["add".into(), "remove".into(), "reload".into()],
        "/tools" => vec!["on".into(), "off".into()],
        "/skills" => vec!["reload".into()],
        "/reasoning" | "/help" | "/examples" | "/list" | "/clear" => vec![],
        "/create" | "/save" | "/model" | "/export" | "/remember" => vec![], // 自由参数
        "/apply" | "/load" | "/rm" => sessions.to_vec(),
        _ => vec![],
    }
}

/// Tab 补全：命令名与参数两级。
/// - 唯一匹配 → 直接补全并加尾随空格；
/// - 多个匹配 → 扩展到最长公共前缀，并在历史里列出候选项；
/// - 无匹配 → 不动。
pub(crate) fn complete_input(app: &mut App) {
    let line = app.input.clone();
    if !line.starts_with('/') || line.contains('\n') {
        return;
    }
    let (cmd, arg_prefix) = match line.split_once(' ') {
        Some((c, rest)) => (c, Some(rest)),
        None => (line.as_str(), None),
    };

    // ---- 第一级：命令名 ----
    let Some(arg_prefix) = arg_prefix else {
        let hits: Vec<String> = COMMANDS
            .iter()
            .filter(|c| c.starts_with(line.as_str()))
            .map(|c| c.to_string())
            .collect();
        apply_completion(app, &line, &hits);
        return;
    };

    // ---- 第二级：参数 ----
    let sessions: Vec<String> = crate::sessions::list(&crate::sessions::sessions_dir())
        .into_iter()
        .map(|s| s.name)
        .collect();
    let hits: Vec<String> = arg_candidates(cmd, &sessions)
        .into_iter()
        .filter(|c| c.starts_with(arg_prefix))
        .collect();
    apply_completion(app, arg_prefix, &hits);
}

/// 把候选项应用到输入框：唯一 → 补全+空格；多个 → 公共前缀 + 提示。
fn apply_completion(app: &mut App, prefix: &str, hits: &[String]) {
    if hits.is_empty() {
        return;
    }
    if hits.len() == 1 {
        app.input = format!(
            "{}{} ",
            &app.input[..app.input.len() - prefix.len()],
            hits[0]
        );
        app.input_cursor = app.input.len();
        return;
    }
    // 扩展最长公共前缀（按字符，兼容中文会话名）。
    let mut common: usize = hits[0].chars().count();
    for h in &hits[1..] {
        let same = h
            .chars()
            .zip(hits[0].chars())
            .take_while(|(a, b)| a == b)
            .count();
        common = common.min(same);
    }
    let common_str: String = hits[0].chars().take(common).collect();
    if common > prefix.chars().count() {
        app.input = format!(
            "{}{}",
            &app.input[..app.input.len() - prefix.len()],
            common_str
        );
        app.input_cursor = app.input.len();
    }
    // 无论是否扩展，都把候选项亮出来供参考。
    let joined: Vec<&str> = hits.iter().map(String::as_str).collect();
    app.push(Role::Hint, format!("候选：{}", joined.join("　")));
}

/// 处理终端事件：键盘输入 / 括号粘贴；审批挂起时只接受 y/n。
pub(crate) fn handle_key(
    ev: Event,
    app: &mut App,
    cmd_tx: &mpsc::UnboundedSender<String>,
    approval_tx: &mpsc::UnboundedSender<Approval>,
) {
    let Event::Key(key) = ev else {
        // 终端粘贴：多行内容折叠为单行插入输入框。
        if let Event::Paste(text) = ev {
            app.input_paste(&text);
        }
        return;
    };
    if key.kind != KeyEventKind::Press {
        return;
    }

    // 审批挂起：仅 y 允许 / n（或 Esc）拒绝，其余忽略。
    if app.pending_approval.is_some() {
        match key.code {
            KeyCode::Char('y' | 'Y') => {
                let id = app.pending_approval.take().unwrap().0;
                let _ = approval_tx.send((id, true));
            }
            KeyCode::Char('n' | 'N') | KeyCode::Esc => {
                let id = app.pending_approval.take().unwrap().0;
                let _ = approval_tx.send((id, false));
            }
            _ => {}
        }
        return;
    }

    match key.code {
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.should_quit = true;
        }
        KeyCode::Esc => {
            // 运行中按 Esc 中止本轮生成（不退出）；空闲时 Esc 退出程序。
            if app.running {
                app.cancel.store(true, Ordering::SeqCst);
            } else {
                app.should_quit = true;
            }
        }
        // 展开/折叠推理摘要：Ctrl-R（Mac 上仍是 Ctrl，而非 Cmd）；显示偏好属纯展示，任何时刻可切换。
        KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.show_reasoning = !app.show_reasoning;
        }
        // 复制最近一次回答到剪贴板（OSC 52）；仅在空闲时，避免打断运行中的流式输出。
        KeyCode::Char('y') if key.modifiers.contains(KeyModifiers::CONTROL) && !app.running => {
            if app.last_answer.trim().is_empty() {
                app.push(Role::Hint, "没有可复制的回答".to_string());
            } else {
                match copy_to_clipboard(&app.last_answer) {
                    Ok(_) => app.push(Role::System, "[已复制最近回答到剪贴板]".to_string()),
                    Err(e) => app.push(Role::Error, format!("✗ 复制失败：{e}")),
                }
            }
        }
        KeyCode::Enter => {
            if app.running {
                // 运行态下忽略提交并给出提示，避免输入静默丢失（用户容易误以为已发送）。
                if !app.input.trim().is_empty() {
                    app.push(Role::Hint, "⏳ 正在运行，请稍候再提交".to_string());
                }
            } else if !app.input.trim().is_empty() {
                let task = app.input.trim().to_string();
                app.input_clear_line();
                app.push(Role::User, format!("❯ {task}"));
                // 控制命令（/ 开头）与退出命令不进入运行态，由 agent 任务直接处理。
                if !task.starts_with('/') && !is_quit_command(&task) {
                    app.running = true;
                }
                let _ = cmd_tx.send(task);
            }
        }
        // ---- 输入框编辑 ----
        KeyCode::Backspace => app.input_backspace(),
        KeyCode::Delete => app.input_delete(),
        KeyCode::Left => app.input_left(),
        KeyCode::Right => app.input_right(),
        KeyCode::Home => app.input_home(),
        KeyCode::End => app.input_end(),
        // Emacs 风格快捷键（Ctrl-A/Ctrl-E/Ctrl-U）。
        KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => app.input_home(),
        KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => app.input_end(),
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.input_clear_line();
        }
        // 翻看历史：运行中也允许（方便回看长输出），不影响任务。
        KeyCode::PageUp => app.scroll_up(),
        KeyCode::PageDown => app.scroll_down(),
        // 命令补全（仅对 / 开头的输入生效）。
        KeyCode::Tab => complete_input(app),
        KeyCode::Char(c) => app.input_push(c),
        _ => {}
    }
}

#[cfg(test)]
#[allow(
    clippy::field_reassign_with_default,
    clippy::collapsible_if,
    clippy::needless_raw_string_hashes
)]
mod input_tests;
