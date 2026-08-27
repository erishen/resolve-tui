//! 会话生命周期命令：/list /create /apply /save /rm。
//!
//! 这些命令都围绕「已归档会话」（git-stash 风格）做增删改查与载入，
//! 与 `info` 子模块里的信息展示/运行时命令解耦。

use std::path::Path;
use tokio::sync::mpsc;

use crate::{
    agent::{AgentEvent, Conversation},
    sessions::{auto_name, delete, list, resolve},
    tui::util::display_path,
};

/// 显式路径兜底（/load 无参数且会话目录为空时）。
const DEFAULT_SESSION: &str = ".resolve-tui-session.json";

/// 列出已归档会话（最新在前）。
pub(crate) async fn cmd_list(tx: &mpsc::UnboundedSender<AgentEvent>, dir: &Path) {
    let items = list(dir);
    if items.is_empty() {
        let _ = tx.send(AgentEvent::System(format!(
            "没有已归档的会话（目录 {}），用 /create <名称> 创建",
            dir.display()
        )));
        return;
    }
    let _ = tx.send(AgentEvent::System(format!(
        "共 {} 个会话（目录 {}）：",
        items.len(),
        dir.display()
    )));
    for s in &items {
        let preview = if s.preview.is_empty() {
            "（空）".to_string()
        } else {
            s.preview.clone()
        };
        let _ = tx.send(AgentEvent::System(format!(
            "[{}] {} · {} · {}",
            s.index, s.name, s.modified, preview
        )));
    }
    let _ = tx.send(AgentEvent::System(
        "用 /apply <索引|名称> 载入。".to_string(),
    ));
}

/// 归档当前对话为新会话（git-stash 语义：清空工作区后从干净状态开始）。
pub(crate) async fn cmd_create(
    conversation: &mut Conversation,
    tx: &mpsc::UnboundedSender<AgentEvent>,
    arg: &str,
    dir: &Path,
) {
    let name = if arg.is_empty() {
        auto_name()
    } else {
        arg.to_string()
    };
    if name.contains('/') {
        let _ = tx.send(AgentEvent::Error(
            "会话名不能含路径分隔符；要存到显式路径请用 /save <路径>".to_string(),
        ));
        return;
    }
    let _ = std::fs::create_dir_all(dir);
    let path_str = dir
        .join(format!("{name}.json"))
        .to_string_lossy()
        .to_string();
    match conversation.save(&path_str) {
        // git-stash 语义：归档当前工作区，然后从干净状态开始。
        Ok(_) => {
            conversation.clear();
            let _ = tx.send(AgentEvent::System(format!(
                "已创建会话 {name} 并清空当前对话（/apply {name} 可切回）"
            )));
        }
        Err(e) => {
            let _ = tx.send(AgentEvent::Error(e.to_string()));
        }
    }
}

/// 载入某个会话继续聊（/load 为不带参数时的 /apply 最近一次）。
pub(crate) async fn cmd_apply(
    conversation: &mut Conversation,
    tx: &mpsc::UnboundedSender<AgentEvent>,
    arg: &str,
    dir: &Path,
) {
    let path = if arg.is_empty() {
        // 不带参数 → 最近一次（列表首项）。
        match list(dir).into_iter().next() {
            Some(s) => s.path.to_string_lossy().to_string(),
            None => DEFAULT_SESSION.to_string(),
        }
    } else if arg.contains('/') {
        arg.to_string()
    } else {
        match resolve(dir, arg) {
            Some(p) => p.to_string_lossy().to_string(),
            None => {
                let _ = tx.send(AgentEvent::Error(format!("未找到会话：{arg}")));
                return;
            }
        }
    };
    match conversation.load(&path) {
        Ok(_) => {
            let _ = tx.send(AgentEvent::System(format!(
                "已载入会话 {}",
                display_path(&path)
            )));
            let _ = tx.send(AgentEvent::Resumed(conversation.input().to_vec()));
        }
        Err(e) => {
            let _ = tx.send(AgentEvent::Error(e.to_string()));
        }
    }
}

/// 给当前对话拍快照（不打断当前会话）。
pub(crate) async fn cmd_save(
    conversation: &mut Conversation,
    tx: &mpsc::UnboundedSender<AgentEvent>,
    arg: &str,
    dir: &Path,
) {
    let path = if arg.is_empty() {
        let _ = std::fs::create_dir_all(dir);
        dir.join(format!("{}.json", auto_name()))
    } else if arg.contains('/') {
        std::path::PathBuf::from(arg)
    } else {
        let _ = std::fs::create_dir_all(dir);
        dir.join(format!("{arg}.json"))
    };
    let path_str = path.to_string_lossy().to_string();
    match conversation.save(&path_str) {
        Ok(_) => {
            let _ = tx.send(AgentEvent::System(format!(
                "已保存快照 {}",
                display_path(&path_str)
            )));
        }
        Err(e) => {
            let _ = tx.send(AgentEvent::Error(e.to_string()));
        }
    }
}

/// 删除某个已归档会话。
pub(crate) async fn cmd_rm(tx: &mpsc::UnboundedSender<AgentEvent>, arg: &str, dir: &Path) {
    if arg.is_empty() || arg.contains('/') {
        let _ = tx.send(AgentEvent::Error("用法：/rm <索引|名称>".to_string()));
    } else {
        match delete(dir, arg) {
            Some(_) => {
                let _ = tx.send(AgentEvent::System(format!("已删除会话 {arg}")));
            }
            None => {
                let _ = tx.send(AgentEvent::Error(format!("未找到会话：{arg}")));
            }
        }
    }
}
