//! 信息展示与运行时命令：/skills /remember /mcp /tools /clear /reasoning
//! /examples /help /model /export，以及未知命令兜底。

use chrono::Local;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

use crate::{
    Config,
    agent::{AgentEvent, Conversation},
    config as harness_config, memory,
    sandbox::{WORKSPACE_RETENTION_SECS, prune_task_workspaces},
    skills,
    tui::util::display_path,
};

/// 查看已加载技能；`/skills reload` 热加载技能目录。
pub(crate) async fn cmd_skills(
    conversation: &mut Conversation,
    tx: &mpsc::UnboundedSender<AgentEvent>,
    arg: &str,
) {
    if arg == "reload" {
        let (skills, skill_warnings) = skills::load_skills(&skills::skills_dir());
        conversation.set_skills(skills);
        for w in &skill_warnings {
            let _ = tx.send(AgentEvent::System(w.clone()));
        }
        let _ = tx.send(AgentEvent::System(format!(
            "已重新加载 {} 个技能",
            conversation.skills().len()
        )));
        return;
    }
    let skills = conversation.skills();
    if skills.is_empty() {
        let _ = tx.send(AgentEvent::System(format!(
            "没有加载任何技能（目录 {}，改完后 /skills reload 生效）",
            skills::skills_dir().display()
        )));
    } else {
        let _ = tx.send(AgentEvent::System(format!(
            "已加载 {} 个技能（目录 {}）：",
            skills.len(),
            skills::skills_dir().display()
        )));
        for s in skills {
            let trig = if s.triggers.is_empty() {
                String::new()
            } else {
                format!(" [触发词: {}]", s.triggers.join("/"))
            };
            let _ = tx.send(AgentEvent::System(format!(
                "  {} — {}{trig}",
                s.name, s.description
            )));
        }
    }
}

/// 长期记忆：`/remember <事实>` 追加；无参查看当前记忆。
pub(crate) async fn cmd_remember(
    _conversation: &mut Conversation,
    tx: &mpsc::UnboundedSender<AgentEvent>,
    arg: &str,
) {
    if arg.is_empty() {
        match memory::memory_context() {
            Some(m) => {
                let path = memory::memory_file()
                    .map(|p| display_path(&p.to_string_lossy()))
                    .unwrap_or_default();
                let _ = tx.send(AgentEvent::System(format!("当前长期记忆（{path}）：\n{m}")));
            }
            None => {
                let _ = tx.send(AgentEvent::System(
                    "记忆为空。用 /remember <事实> 添加，例如：\n/remember 部署前先跑 cargo fmt"
                        .to_string(),
                ));
            }
        }
    } else {
        match memory::remember(arg) {
            Ok(p) => {
                let _ = tx.send(AgentEvent::System(format!(
                    "已记住（写入 {}）",
                    display_path(&p.to_string_lossy())
                )));
            }
            Err(e) => {
                let _ = tx.send(AgentEvent::Error(e.to_string()));
            }
        }
    }
}

/// MCP server 管理：add / remove / reload，无参查看状态。
pub(crate) async fn cmd_mcp(
    conversation: &mut Conversation,
    tx: &mpsc::UnboundedSender<AgentEvent>,
    arg: &str,
    config: &Config,
) {
    let (sub, rest) = match arg.split_once(' ') {
        Some((s, r)) => (s.trim(), r.trim()),
        None => (arg, ""),
    };
    match sub {
        "add" => {
            let mut parts = rest.split_whitespace();
            let Some(name) = parts.next() else {
                let _ = tx.send(AgentEvent::Error(
                    "用法：/mcp add <名字> <命令> [参数…]（例：/mcp add fs npx -y @modelcontextprotocol/server-filesystem /tmp）"
                        .to_string(),
                ));
                return;
            };
            let Some(command) = parts.next() else {
                let _ = tx.send(AgentEvent::Error(
                    "缺少命令：/mcp add <名字> <命令> [参数…]".to_string(),
                ));
                return;
            };
            let args: Vec<String> = parts.map(str::to_string).collect();
            match conversation.add_mcp(name, command, &args).await {
                Ok(n) => {
                    // 持久化到 config.toml（保留原文件排版，仅追加）。
                    match harness_config::config_file() {
                        Some(path) => {
                            if let Err(e) =
                                harness_config::append_mcp_server(&path, name, command, &args)
                            {
                                let _ = tx.send(AgentEvent::Error(format!(
                                    "已连接但写入配置失败：{e}（重启后会丢失）"
                                )));
                            }
                        }
                        None => {
                            let _ = tx.send(AgentEvent::Error(
                                "已连接，但找不到配置文件路径（未设 HARNESS_CONFIG）".to_string(),
                            ));
                        }
                    }
                    let _ = tx.send(AgentEvent::System(format!(
                        "已挂载 {name}（{n} 个工具），下一轮对话即可使用；已写入 config.toml"
                    )));
                }
                Err(e) => {
                    let _ = tx.send(AgentEvent::Error(format!("挂载 {name} 失败：{e}")));
                }
            }
        }
        "remove" => {
            if rest.is_empty() {
                let _ = tx.send(AgentEvent::Error("用法：/mcp remove <名字>".to_string()));
                return;
            }
            match conversation.remove_mcp(rest).await {
                Ok(()) => {
                    if let Some(path) = harness_config::config_file()
                        && let Err(e) = harness_config::remove_mcp_server(&path, rest)
                    {
                        let _ = tx.send(AgentEvent::Error(format!("已摘除但更新配置失败：{e}")));
                    }
                    let _ = tx.send(AgentEvent::System(format!(
                        "已移除 server {rest}（工具即刻失效，配置已同步删除）"
                    )));
                }
                Err(e) => {
                    let _ = tx.send(AgentEvent::Error(e.to_string()));
                }
            }
        }
        "reload" => {
            conversation.reconnect_mcp(config).await;
            let n = conversation
                .visible_tools()
                .iter()
                .filter(|(name, _, _)| name.starts_with("mcp_"))
                .count();
            let _ = tx.send(AgentEvent::System(format!("MCP 已重连（{n} 个远端工具）")));
            for l in conversation.mcp_status() {
                let _ = tx.send(AgentEvent::System(format!("  {l}")));
            }
        }
        _ => {
            let status = conversation.mcp_status();
            if status.is_empty() {
                let _ = tx.send(AgentEvent::System(
                    "未配置 MCP server；/mcp add <名> <命令> [参数…] 可动态添加，或编辑 config.toml 后 /mcp reload"
                        .to_string(),
                ));
            } else {
                let _ = tx.send(AgentEvent::System("MCP server 状态：".to_string()));
                for l in status {
                    let _ = tx.send(AgentEvent::System(format!("  {l}")));
                }
            }
        }
    }
}

/// 查看工具与启停开关：`/tools on|off <名>`。
pub(crate) async fn cmd_tools(
    conversation: &mut Conversation,
    tx: &mpsc::UnboundedSender<AgentEvent>,
    arg: &str,
) {
    let (action, name) = match arg.split_once(' ') {
        Some((a, n)) => (a.trim(), n.trim()),
        None => ("", arg),
    };
    if action.is_empty() && name.is_empty() {
        let tools = conversation.visible_tools();
        let _ = tx.send(AgentEvent::System(format!("可用工具（{}）：", tools.len())));
        for (name, desc, enabled) in tools {
            let mark = if enabled { "✓" } else { "✗（已禁用）" };
            let _ = tx.send(AgentEvent::System(format!("  {mark} {name} — {desc}")));
        }
        return;
    }
    match action {
        "on" | "off" => {
            if name.is_empty() {
                let _ = tx.send(AgentEvent::Error(format!("用法：/tools {action} <工具名>")));
            } else if conversation.set_tool_enabled(name, action == "on") {
                let _ = tx.send(AgentEvent::System(format!(
                    "已{}工具 {name}",
                    if action == "on" { "启用" } else { "禁用" }
                )));
            } else {
                let _ = tx.send(AgentEvent::Error(format!("未知工具: {name}")));
            }
        }
        _ => {
            let _ = tx.send(AgentEvent::Error(
                "用法：/tools 或 /tools on|off <工具名>".to_string(),
            ));
        }
    }
}

/// 清空当前对话。
pub(crate) async fn cmd_clear(
    conversation: &mut Conversation,
    tx: &mpsc::UnboundedSender<AgentEvent>,
) {
    conversation.clear();
    let _ = tx.send(AgentEvent::ClearScreen);
    let _ = tx.send(AgentEvent::System("已清空当前会话".to_string()));
}

/// 退出 TUI（/quit /exit /q，或裸写 q/exit/quit）。
/// 仅发一个 `Quit` 事件——真正的退出由 UI 事件循环据此置位 `should_quit`，
/// 与 Ctrl-C / 空闲 Esc 走同一条退出路径（退出前自动存档当前会话）。
pub(crate) async fn cmd_quit(tx: &mpsc::UnboundedSender<AgentEvent>) {
    let _ = tx.send(AgentEvent::System("再见 👋（会话已自动存档）".to_string()));
    let _ = tx.send(AgentEvent::Quit);
}

/// 切换推理过程展示。
pub(crate) async fn cmd_reasoning(tx: &mpsc::UnboundedSender<AgentEvent>) {
    let _ = tx.send(AgentEvent::ToggleReasoning);
}

/// 输出内置示例文档（Markdown 渲染）。文件随 crate 安装。
pub(crate) async fn cmd_examples(tx: &mpsc::UnboundedSender<AgentEvent>) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/examples.md");
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            let _ = tx.send(AgentEvent::Document(text));
        }
        Err(e) => {
            let _ = tx.send(AgentEvent::Error(format!(
                "读取 {} 失败: {e}",
                path.display()
            )));
        }
    }
}

/// 帮助：列出全部命令与快捷键。
pub(crate) async fn cmd_help(tx: &mpsc::UnboundedSender<AgentEvent>) {
    let lines = [
        "命令：",
        "  /list              列出已归档会话（git-stash 风格）",
        "  /create [名称]      归档当前对话为会话并清空，开始新对话",
        "  /apply <索引|名称>  载入某个会话继续聊（同 /load）",
        "  /save [名称|路径]   给当前对话拍快照（不打断）",
        "  /load              同 /apply（载入最近一次）",
        "  /clear             清空当前对话",
        "  /rm <索引|名称>     删除某个会话",
        "  /model [名称]       切换模型（无参查看当前）",
        "  /pse [on|off]       切换多 Agent 三角色模式（无参切换；运行时生效）",
        "  /sandbox [clean]     查看沙箱工作区 / 列出任务目录；clean 立即清空",
        "  /reasoning         切换推理过程展示（或 Ctrl-R）",
        "  /export [路径]      导出当前会话为 Markdown",
        "  /tools [on|off 名]  查看 / 启停工具（内置 + MCP）",
        "  /skills [reload]    查看技能；reload 热加载技能目录",
        "  /remember [事实]    长期记忆：无参查看；带参追加（跨会话生效）",
        "  /examples          输出工具使用示例（Markdown）",
        "  /mcp [add|remove|reload]",
        "                      MCP 状态；add <名> <命令> [参数…] 动态挂载并存盘",
        "  /quit | /exit | /q  退出（可裸写 q / exit / quit），退出自动存档",
        "  /help              显示本帮助",
        "快捷键：",
        "  Enter 提交 · PageUp/PageDown 翻历史 · Ctrl-R 推理 · Ctrl-Y 复制回答",
        "  运行中 Esc 中止生成 · 空闲 Esc / Ctrl-C 退出",
        "退出自动存档、启动自动续接上次会话。",
    ];
    for l in lines {
        let _ = tx.send(AgentEvent::System(l.to_string()));
    }
}

/// 查看 / 切换当前模型。
pub(crate) async fn cmd_model(
    model: &Arc<Mutex<String>>,
    tx: &mpsc::UnboundedSender<AgentEvent>,
    arg: &str,
) {
    if arg.is_empty() {
        let current = model.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let _ = tx.send(AgentEvent::System(format!("当前模型：{current}")));
    } else {
        if let Ok(mut g) = model.lock() {
            *g = arg.to_string();
        }
        let _ = tx.send(AgentEvent::System(format!("已切换到模型：{arg}")));
    }
}

/// 运行时切换多 Agent（PSE：Planner/Specialist/Evaluator）模式。`/pse` 切换，
/// `/pse on|off` 显式设定；下一轮对话即生效（无需重启）。
pub(crate) async fn cmd_pse(
    pse: &Arc<AtomicBool>,
    tx: &mpsc::UnboundedSender<AgentEvent>,
    arg: &str,
) {
    let cur = pse.load(Ordering::SeqCst);
    let next = match arg.to_ascii_lowercase().as_str() {
        "on" | "1" | "true" | "yes" => true,
        "off" | "0" | "false" | "no" => false,
        "" => !cur,
        _ => {
            let _ = tx.send(AgentEvent::System(format!(
                "[PSE] 用法：/pse（切换）| /pse on | /pse off（当前：{}）",
                if cur { "on" } else { "off" }
            )));
            return;
        }
    };
    pse.store(next, Ordering::SeqCst);
    let _ = tx.send(AgentEvent::System(format!(
        "[PSE] 多 Agent 三角色模式：{}",
        if next {
            "on（Planner 规划 → Specialist 执行 → Evaluator 验证）"
        } else {
            "off（单 agent）"
        }
    )));
}

/// 查看沙箱工作区状态 / 列出任务目录；`/sandbox clean` 立即清空全部任务工作区。
pub(crate) async fn cmd_sandbox(
    config: &Config,
    tx: &mpsc::UnboundedSender<AgentEvent>,
    arg: &str,
) {
    use std::time::SystemTime;

    let send = |msg: String| {
        let _ = tx.send(AgentEvent::System(msg));
    };
    if !config.policy.enabled {
        send("[sandbox] 沙箱已关闭（HARNESS_SANDBOX=0），无工作区。".to_string());
        return;
    }
    let root = &config.sandbox_dir;
    if arg == "clean" {
        let n = prune_task_workspaces(root, 0);
        send(format!("[sandbox] 已清理 {n} 个任务工作区"));
        return;
    }
    if !arg.is_empty() {
        send(format!(
            "[sandbox] 用法：/sandbox 查看 | /sandbox clean 立即清空（当前参数 \"{arg}\" 被忽略）"
        ));
    }
    send(format!(
        "[sandbox] 根目录: {} | 保留 {} 天",
        root.display(),
        WORKSPACE_RETENTION_SECS / 86400
    ));
    let now = SystemTime::now();
    let Ok(rd) = std::fs::read_dir(root) else {
        send("  （目录不存在）".to_string());
        return;
    };
    let mut tasks: Vec<(String, String)> = rd
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            if !name.starts_with("task-") {
                return None;
            }
            let age = e
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| now.duration_since(t).ok())
                .map(|d| {
                    if d.as_secs() < 3600 {
                        format!("{} 分钟前", d.as_secs() / 60)
                    } else if d.as_secs() < 86400 {
                        format!("{} 小时前", d.as_secs() / 3600)
                    } else {
                        format!("{} 天前", d.as_secs() / 86400)
                    }
                })
                .unwrap_or_else(|| "未知".to_string());
            Some((name, age))
        })
        .collect();
    tasks.sort();
    if tasks.is_empty() {
        send("  （暂无任务工作区）".to_string());
    } else {
        send(format!("  共 {} 个任务工作区：", tasks.len()));
        for (name, age) in tasks {
            send(format!("  {name}（{age}）"));
        }
    }
}

/// 导出当前会话为 Markdown。
pub(crate) async fn cmd_export(tx: &mpsc::UnboundedSender<AgentEvent>, arg: &str) {
    let path = if arg.is_empty() {
        let stamp = Local::now().format("%Y%m%d-%H%M%S");
        format!("resolve-tui-export-{stamp}.md")
    } else {
        arg.to_string()
    };
    let _ = tx.send(AgentEvent::Export(path));
}

/// 未知命令兜底提示。
pub(crate) async fn cmd_unknown(tx: &mpsc::UnboundedSender<AgentEvent>, cmd: &str) {
    let _ = tx.send(AgentEvent::System(format!(
        "未知命令：{cmd}（可用 /list /create /apply /save /load /clear /rm /reasoning /export /model /pse /sandbox /help，退出用 /quit 或 q）"
    )));
}
