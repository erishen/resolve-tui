use super::*;
use crate::model::InputItem;
use crate::sessions::sessions_dir;
use crate::tui::commands::handle_control;
use crate::{Config, agent::AgentEvent, agent::Conversation};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

// 验证 `/save` 控制命令：会触发序列化写盘并广播 System 事件。
#[tokio::test]
async fn control_save_writes_file_and_emits_system() {
    let (tx, mut rx) = mpsc::unbounded_channel::<AgentEvent>();
    let mut conv = Conversation::new();
    conv.input_mut().push(InputItem::message("user", "你好"));
    let path = std::env::temp_dir().join("tui_save_test.json");
    let p = path.to_str().unwrap().to_string();
    handle_control(
        &mut conv,
        &tx,
        &format!("/save {p}"),
        &Arc::new(Mutex::new(String::new())),
        &Config::from_env(),
        &Arc::new(AtomicBool::new(false)),
    )
    .await;

    let mut saw_system = false;
    while let Ok(ev) = rx.try_recv() {
        if let AgentEvent::System(s) = ev {
            assert!(s.contains("已保存"), "unexpected system msg: {s}");
            saw_system = true;
        }
    }
    assert!(saw_system, "expected a System event");
    assert!(path.exists(), "session file should be created");
    let _ = std::fs::remove_file(&path);
}

// 验证未知控制命令也能给出 System 提示而非崩溃。
#[tokio::test]
async fn control_unknown_emits_system() {
    let (tx, mut rx) = mpsc::unbounded_channel::<AgentEvent>();
    let mut conv = Conversation::new();
    handle_control(
        &mut conv,
        &tx,
        "/bogus",
        &Arc::new(Mutex::new(String::new())),
        &Config::from_env(),
        &Arc::new(AtomicBool::new(false)),
    )
    .await;
    match rx.try_recv().unwrap() {
        AgentEvent::System(s) => assert!(s.contains("未知命令")),
        other => panic!("expected System, got {other:?}"),
    }
}

// /model <名> 应切换共享模型；/model 无参应回显当前模型。
#[tokio::test]
async fn control_model_switches_shared_model() {
    let (tx, mut rx) = mpsc::unbounded_channel::<AgentEvent>();
    let mut conv = Conversation::new();
    let model = Arc::new(Mutex::new("gpt-4o-mini".to_string()));

    handle_control(
        &mut conv,
        &tx,
        "/model agnes-x",
        &model,
        &Config::from_env(),
        &Arc::new(AtomicBool::new(false)),
    )
    .await;
    assert_eq!(model.lock().unwrap().as_str(), "agnes-x");
    match rx.try_recv().unwrap() {
        AgentEvent::System(s) => assert!(s.contains("已切换"), "unexpected: {s}"),
        other => panic!("expected System, got {other:?}"),
    }

    handle_control(
        &mut conv,
        &tx,
        "/model",
        &model,
        &Config::from_env(),
        &Arc::new(AtomicBool::new(false)),
    )
    .await;
    match rx.try_recv().unwrap() {
        AgentEvent::System(s) => assert!(s.contains("agnes-x"), "unexpected: {s}"),
        other => panic!("expected System, got {other:?}"),
    }
}

// /help 应输出多行帮助，且不被当作未知命令。
#[tokio::test]
async fn control_help_lists_commands() {
    let (tx, mut rx) = mpsc::unbounded_channel::<AgentEvent>();
    let mut conv = Conversation::new();
    let model = Arc::new(Mutex::new(String::new()));
    handle_control(
        &mut conv,
        &tx,
        "/help",
        &model,
        &Config::from_env(),
        &Arc::new(AtomicBool::new(false)),
    )
    .await;

    let mut lines = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        match ev {
            AgentEvent::System(s) => lines.push(s),
            other => panic!("expected System, got {other:?}"),
        }
    }
    assert!(lines.len() >= 5, "help should be multi-line");
    assert!(lines.iter().any(|l| l.contains("命令：")));
    assert!(lines.iter().any(|l| l.contains("/export")));
    assert!(!lines.iter().any(|l| l.contains("未知命令")));
}

// /tools 列表与 on/off 开关：禁用的工具应从提交给模型的列表中剔除。
#[tokio::test]
async fn control_tools_lists_and_toggles() {
    let (tx, mut rx) = mpsc::unbounded_channel::<AgentEvent>();
    let mut conv = Conversation::new();
    let model = Arc::new(Mutex::new(String::new()));
    let cfg = Config::from_env();

    handle_control(
        &mut conv,
        &tx,
        "/tools",
        &model,
        &cfg,
        &Arc::new(AtomicBool::new(false)),
    )
    .await;
    let mut listed = 0;
    while let Ok(ev) = rx.try_recv() {
        if let AgentEvent::System(s) = ev {
            if s.contains("shell") {
                listed += 1;
            }
        }
    }
    assert!(listed >= 1, "/tools 应列出内置 shell 工具");

    handle_control(
        &mut conv,
        &tx,
        "/tools off shell",
        &model,
        &cfg,
        &Arc::new(AtomicBool::new(false)),
    )
    .await;
    let (_, _, enabled) = *conv
        .visible_tools()
        .iter()
        .find(|(n, _, _)| n == "shell")
        .unwrap();
    assert!(!enabled, "off 后 shell 应为禁用态");

    // 禁用名单只影响展示与请求；再 on 回来即恢复。
    handle_control(
        &mut conv,
        &tx,
        "/tools on shell",
        &model,
        &cfg,
        &Arc::new(AtomicBool::new(false)),
    )
    .await;
    assert!(
        conv.visible_tools()
            .iter()
            .all(|(n, _, e)| n != "shell" || *e),
        "on 后 shell 应恢复启用"
    );

    // 未知名要报错；坏用法也要报错（先清掉前面命令残留的提示消息）。
    while rx.try_recv().is_ok() {}
    handle_control(
        &mut conv,
        &tx,
        "/tools off no-such-tool",
        &model,
        &cfg,
        &Arc::new(AtomicBool::new(false)),
    )
    .await;
    match rx.try_recv().unwrap() {
        AgentEvent::Error(s) => assert!(s.contains("未知工具")),
        other => panic!("expected Error, got {other:?}"),
    }
}

// /mcp reload 在未配置任何 server 时应给出空状态而非崩溃。
#[tokio::test]
async fn control_mcp_reload_without_servers_is_safe() {
    let (tx, mut rx) = mpsc::unbounded_channel::<AgentEvent>();
    let mut conv = Conversation::new();
    handle_control(
        &mut conv,
        &tx,
        "/mcp reload",
        &Arc::new(Mutex::new(String::new())),
        &Config::from_env(),
        &Arc::new(AtomicBool::new(false)),
    )
    .await;
    let mut saw_any = false;
    while let Ok(ev) = rx.try_recv() {
        if matches!(ev, AgentEvent::System(_)) {
            saw_any = true;
        }
    }
    assert!(saw_any, "reload 后应有状态提示");
    assert!(conv.mcp_status().is_empty());
}

// Ctrl-Y：无回答时应提示，有回答时应写入剪贴板（OSC 52 写 stdout，测试环境被捕获不影响）。
#[test]
fn ctrl_y_copies_last_answer() {
    let (cmd_tx, _) = mpsc::unbounded_channel::<String>();
    let (approval_tx, _) = mpsc::unbounded_channel::<Approval>();

    let mut empty = App::default();
    handle_key(
        crossterm::event::Event::Key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('y'),
            crossterm::event::KeyModifiers::CONTROL,
        )),
        &mut empty,
        &cmd_tx,
        &approval_tx,
    );
    let last = empty
        .scrollback
        .last()
        .map(|r| r.text())
        .unwrap_or_default();
    assert!(last.contains("没有可复制的回答"), "unexpected: {last}");

    let mut filled = App::default();
    filled.last_answer = "hello world".to_string();
    handle_key(
        crossterm::event::Event::Key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('y'),
            crossterm::event::KeyModifiers::CONTROL,
        )),
        &mut filled,
        &cmd_tx,
        &approval_tx,
    );
    let last = filled
        .scrollback
        .last()
        .map(|r| r.text())
        .unwrap_or_default();
    assert!(last.contains("已复制"), "unexpected: {last}");
}

// 审批挂起时，y 键应把应答（id, true）发出并清空 pending。
#[test]
fn pending_approval_y_sends_true() {
    let mut app = App::default();
    app.pending_approval = Some(("c1".to_string(), "shell".to_string(), "ls".to_string()));
    let (approval_tx, mut approval_rx) = mpsc::unbounded_channel::<Approval>();
    let ev = crossterm::event::Event::Key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Char('y'),
        crossterm::event::KeyModifiers::empty(),
    ));
    handle_key(
        ev,
        &mut app,
        &mpsc::unbounded_channel::<String>().0,
        &approval_tx,
    );
    assert!(app.pending_approval.is_none());
    let decision = approval_rx.try_recv().expect("decision sent");
    assert_eq!(decision, ("c1".to_string(), true));
}

// git-stash 往返：/create 归档并清空，/apply 按名称切回。
#[tokio::test]
async fn create_then_apply_roundtrip() {
    let (tx, mut rx) = mpsc::unbounded_channel::<AgentEvent>();
    let mut conv = Conversation::new();
    conv.input_mut()
        .push(InputItem::message("user", "roundtrip 内容"));

    handle_control(
        &mut conv,
        &tx,
        "/create zz-test-create",
        &Arc::new(Mutex::new(String::new())),
        &Config::from_env(),
        &Arc::new(AtomicBool::new(false)),
    )
    .await;
    assert!(conv.input().is_empty(), "/create 后应清空当前对话");

    handle_control(
        &mut conv,
        &tx,
        "/apply zz-test-create",
        &Arc::new(Mutex::new(String::new())),
        &Config::from_env(),
        &Arc::new(AtomicBool::new(false)),
    )
    .await;
    let file = sessions_dir().join("zz-test-create.json");
    let _ = std::fs::remove_file(&file);

    let json = serde_json::to_value(conv.input()).unwrap();
    assert_eq!(json[0]["type"], "message");
    assert_eq!(json[0]["content"][0]["text"], "roundtrip 内容");

    let mut msgs = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        if let AgentEvent::System(s) = ev {
            msgs.push(s);
        }
    }
    assert!(msgs.iter().any(|m| m.contains("已创建")), "msgs={msgs:?}");
    assert!(msgs.iter().any(|m| m.contains("已载入")), "msgs={msgs:?}");
}

// Tab 补全：命令名唯一补全、多候选提示、参数级补全、非 / 输入不动。
#[test]
fn tab_completion_completes_commands_and_args() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let tab = || crossterm::event::Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::empty()));
    let (cmd_tx, _) = mpsc::unbounded_channel::<String>();
    let (approval_tx, _) = mpsc::unbounded_channel::<Approval>();
    let run = |app: &mut App| handle_key(tab(), app, &cmd_tx, &approval_tx);

    // 唯一匹配 → 补全并加尾随空格。
    let mut app = App::default();
    app.input = "/too".into();
    run(&mut app);
    assert_eq!(app.input, "/tools ");
    assert_eq!(app.input_cursor, app.input.len());

    // 多候选（/m）→ 无唯一补全，但列出候选项。
    let mut app = App::default();
    app.input = "/m".into();
    run(&mut app);
    assert_eq!(app.input, "/m", "无公共前缀可扩展时不应改动");
    let last: Vec<String> = app.scrollback.iter().map(|r| r.text()).collect();
    assert!(
        last.iter()
            .any(|t| t.contains("/model") && t.contains("/mcp")),
        "应列出候选"
    );

    // 参数级：/tools of → off（唯一匹配）。
    let mut app = App::default();
    app.input = "/tools of".into();
    run(&mut app);
    assert_eq!(app.input, "/tools off ");

    // 参数级：/mcp 空参 → 列出 add/remove/reload 候选。
    let mut app = App::default();
    app.input = "/mcp ".into();
    run(&mut app);
    let last: Vec<String> = app.scrollback.iter().map(|r| r.text()).collect();
    assert!(
        last.iter()
            .any(|t| t.contains("add") && t.contains("remove")),
        "应列出 mcp 子命令"
    );

    // 非 / 输入不受影响。
    let mut app = App::default();
    app.input = "hello".into();
    app.input_cursor = 5;
    run(&mut app);
    assert_eq!(app.input, "hello");

    // 会话名补全：临时会话目录里放一个文件后 /apply zz 应能补全。
    let dir = std::env::temp_dir().join(format!("harness_tabcomp_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("zz-demo.json"), "{}").unwrap();
    unsafe { std::env::set_var("HARNESS_SESSIONS_DIR", &dir) };
    let mut app = App::default();
    app.input = "/apply zz".into();
    run(&mut app);
    assert_eq!(app.input, "/apply zz-demo ");
    unsafe { std::env::remove_var("HARNESS_SESSIONS_DIR") };
    let _ = std::fs::remove_dir_all(&dir);
}
