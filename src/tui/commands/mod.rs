//! 会话控制命令（/list /create /apply /save /load /clear /rm /skills /remember
//! /mcp /tools /model /pse /export /reasoning /examples /help）。
//!
//! `handle_control` 只负责「解析命令字 + 参数」，再转发给独立的 `cmd_*`
//! 处理函数（见 `session` / `info` 子模块）——每个命令自成一函数，便于单独测试与扩展；
//! 所有结果都通过 `AgentEvent` 广播回 TUI 事件循环。

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;

use crate::{
    Config,
    agent::{AgentEvent, Conversation},
    sessions::sessions_dir,
};

mod info;
mod session;

// 命令实现拆到 `session` / `info` 子模块；这里统一再导出，使分发器可直接按名调用。
pub(crate) use info::*;
pub(crate) use session::*;

/// 判断一行是否为「退出命令」：`/quit` `/exit` `/q` 与裸写的 `q` `exit` `quit`
/// 都识别为退出（大小写不敏感）。TUI 事件循环在把输入当作任务提交前，
/// 先用它识别退出意图并交给 `handle_control` 处理。
pub(crate) fn is_quit_command(line: &str) -> bool {
    matches!(
        line.trim().to_ascii_lowercase().as_str(),
        "/q" | "/quit" | "/exit" | "q" | "quit" | "exit"
    )
}

/// 解析命令字与参数，转发到各 `cmd_*` 处理函数。
pub(crate) async fn handle_control(
    conversation: &mut Conversation,
    tx: &mpsc::UnboundedSender<AgentEvent>,
    line: &str,
    model: &Arc<Mutex<String>>,
    config: &Config,
    pse: &Arc<AtomicBool>,
) {
    let mut parts = line.splitn(2, ' ');
    let cmd = parts.next().unwrap_or("");
    let arg = parts.next().unwrap_or("").trim();
    let dir = sessions_dir();

    match cmd {
        "/list" => cmd_list(tx, &dir).await,
        "/create" => cmd_create(conversation, tx, arg, &dir).await,
        "/load" | "/apply" => cmd_apply(conversation, tx, arg, &dir).await,
        "/save" => cmd_save(conversation, tx, arg, &dir).await,
        "/skills" => cmd_skills(conversation, tx, arg).await,
        "/remember" => cmd_remember(conversation, tx, arg).await,
        "/mcp" => cmd_mcp(conversation, tx, arg, config).await,
        "/tools" => cmd_tools(conversation, tx, arg).await,
        "/clear" => cmd_clear(conversation, tx).await,
        "/reasoning" => cmd_reasoning(tx).await,
        "/examples" => cmd_examples(tx).await,
        "/help" => cmd_help(tx).await,
        "/model" => cmd_model(model, tx, arg).await,
        "/pse" => cmd_pse(pse, tx, arg).await,
        "/sandbox" => cmd_sandbox(config, tx, arg).await,
        "/export" => cmd_export(tx, arg).await,
        "/rm" => cmd_rm(tx, arg, &dir).await,
        "/quit" | "/exit" | "/q" | "quit" | "exit" | "q" => cmd_quit(tx).await,
        _ => cmd_unknown(tx, cmd).await,
    }
}

#[cfg(test)]
mod tests {
    use super::is_quit_command;

    #[test]
    fn quit_commands_all_recognized() {
        for c in ["/quit", "/exit", "/q", "quit", "exit", "q"] {
            assert!(is_quit_command(c), "{c} 应被识别为退出命令");
        }
    }

    #[test]
    fn quit_commands_case_insensitive_and_trimmed() {
        for c in ["/Quit", "EXIT", "  q  ", "Quit"] {
            assert!(is_quit_command(c), "{c} 应被识别为退出命令（大小写/空白）");
        }
    }

    #[test]
    fn non_quit_inputs_rejected() {
        for c in [
            "",
            "quit now",
            "q.py",
            "question",
            "/quitx",
            "explain exit",
            "/help",
        ] {
            assert!(!is_quit_command(c), "{c} 不应被识别为退出命令");
        }
    }
}
