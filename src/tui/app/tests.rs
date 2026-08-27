//! `App` 的单元测试：导出、翻页、输入编辑、Markdown 渲染、事件映射等。

#![allow(clippy::field_reassign_with_default)]

use super::*;
use crate::tui::format::{capability_lines, display_width_pub};

// 导出 Markdown：应包含对话记录与推理过程。
#[test]
fn export_markdown_writes_transcript() {
    let mut app = App::default();
    app.push(Role::User, "❯ 你好".to_string());
    app.push(Role::System, "Hi there!".to_string());
    app.reasoning.push("先看看环境".to_string());

    let path = std::env::temp_dir().join("tui_export_test.md");
    app.export_markdown(path.to_str().unwrap())
        .expect("export failed");
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("# resolve-tui 会话导出"));
    assert!(content.contains("❯ 你好"));
    assert!(content.contains("Hi there!"));
    assert!(content.contains("## 推理过程"));
    assert!(content.contains("先看看环境"));
    let _ = std::fs::remove_file(&path);
}

// 翻页偏移：向上累计但最多到顶，向下归零即跟随最新；历史超上限会被裁剪。
#[test]
fn scroll_offset_clamped_and_history_capped() {
    let mut app = App::default();
    app.viewport_h = 10;
    for i in 0..50 {
        app.push(Role::System, format!("line {i}"));
    }
    assert_eq!(app.scroll_offset, 0);

    for _ in 0..100 {
        app.scroll_up();
    }
    assert_eq!(
        app.scroll_offset,
        app.scrollback.len().saturating_sub(1),
        "向上翻页应被钳制到顶部"
    );

    for _ in 0..100 {
        app.scroll_down();
    }
    assert_eq!(app.scroll_offset, 0, "向下翻到底应恢复跟随最新");

    for i in 0..(MAX_SCROLLBACK + 100) {
        app.push(Role::System, format!("old {i}"));
    }
    assert_eq!(app.scrollback.len(), MAX_SCROLLBACK, "历史应被裁剪到上限");
}

// 输入编辑：光标移动/插入/删除都应在字符边界上，不切断中文。
#[test]
fn input_editing_respects_char_boundaries() {
    let mut app = App::default();
    for c in "你好".chars() {
        app.input_push(c);
    }
    assert_eq!(app.input, "你好");
    assert_eq!(app.input_cursor, app.input.len());

    // 在「你」和「好」之间插入 X。
    app.input_left();
    app.input_push('X');
    assert_eq!(app.input, "你X好");

    // 退格删掉 X。
    app.input_backspace();
    assert_eq!(app.input, "你好");

    // 移到最左再退格是空操作（光标已在开头）。
    app.input_home();
    app.input_backspace();
    assert_eq!(app.input, "你好");
    assert_eq!(app.input_cursor, 0);

    // 移到末尾并退格，应删掉「好」。
    app.input_end();
    app.input_backspace();
    assert_eq!(app.input, "你");
    assert_eq!(app.input_cursor, 3);

    // 光标在开头时 Delete 删除光标处字符。
    app.input_home();
    app.input_delete();
    assert_eq!(app.input, "");
    assert_eq!(app.input_cursor, 0);
}

// 粘贴：换行折叠为空格，控制字符丢弃。
#[test]
fn paste_folds_newlines_and_strips_controls() {
    let mut app = App::default();
    app.input_paste("ab\ncd\r\nef");
    assert_eq!(app.input, "ab cd ef");

    let mut app2 = App::default();
    app2.input_paste("\u{3}\u{7}");
    assert_eq!(app2.input, "");
}

// 能力明细：概览行 + 统一两空格缩进的分类明细，日志左缘对齐。
#[test]
fn capability_lines_lists_concrete_items() {
    use crate::skills::Skill;
    let tools = vec![
        ("shell".to_string(), "沙箱执行".to_string(), true),
        ("mcp_fs_read".to_string(), "读文件".to_string(), true),
    ];
    let skills = vec![Skill {
        name: "rust-review".to_string(),
        description: "代码评审".to_string(),
        triggers: vec!["review".to_string(), "审查".to_string()],
        body: "body".to_string(),
        ..Default::default()
    }];
    let mcp = vec!["fs：已连接（1 个工具）".to_string()];

    let lines = capability_lines(&tools, &skills, &mcp);
    // 概览行顶格；明细行一律两空格缩进。
    assert!(lines[0].starts_with("能力概览｜工具 2"));
    assert!(lines[0].contains("MCP 在线 1"));
    for l in &lines[1..] {
        assert!(l.starts_with("  "), "明细行应缩进: {l:?}");
    }
    // 工具清单不在启动日志展开（明细走 /tools），只保留 MCP 状态与技能。
    assert!(!lines.iter().any(|l| l.contains("shell")));
    assert!(lines.iter().any(|l| l.starts_with("  MCP fs：已连接")));
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("  技能 rust-review") && l.contains("review")),
        "技能应列出名字与触发词: {lines:?}"
    );
}

// 表格：GFM 管道表应渲染为等宽框线，且各行显示宽一致（列对齐）。
#[test]
fn format_answer_renders_gfm_table_as_box() {
    let md = "| 主题 | 内容概要 |\n|---|---|\n| 全市场股票技术扫描 | 51 个指标 × 5000 只股票 |\n| Agent Harness | 多智能体编排 |\n";
    let rows = format_answer(md, Color::White, Color::Cyan);
    // 上边框 + 表头 + 分隔线 + 2 数据行 + 下边框。
    assert_eq!(
        rows.len(),
        6,
        "{:?}",
        rows.iter().map(|r| r.text()).collect::<Vec<_>>()
    );

    let texts: Vec<String> = rows.iter().map(|r| r.text()).collect();
    // 框线行存在且宽度一致。
    assert!(texts[0].starts_with('┌') && texts[0].ends_with('┐'));
    assert!(texts[2].starts_with('├') && texts[5].starts_with('└'));
    let w0 = display_width_pub(&texts[0]);
    for t in &texts {
        assert_eq!(display_width_pub(t), w0, "所有框线行应等宽: {t:?}");
    }
    // 单元格内容保留、竖线对齐（每行含相同数量的 │）。
    // 内容行（表头+数据，索引 1/3/4）竖线数量一致 = 列对齐；框线行用 ┬┼┴ 不含 │。
    for i in [1usize, 3, 4] {
        assert_eq!(
            texts[i].matches('│').count(),
            3,
            "竖线应等距对齐: {:?}",
            texts[i]
        );
    }
    assert!(texts[1].contains("主题") && texts[1].contains("内容概要"));
    assert!(texts[3].contains("全市场股票技术扫描"));
    assert!(texts[4].contains("Agent Harness"));

    // 非表格文本保持原样一行。
    let plain = format_answer("普通回答\n第二行", Color::White, Color::Cyan);
    assert_eq!(plain.len(), 2);
    assert_eq!(plain[0].text(), "普通回答");

    // 前导空行（部分模型输出 \n\n 开头）由 flush_answer 裁剪，这里验证 format_answer 不再放大问题：
    // 空串输入只产出一个空行而非多行。
    assert_eq!(format_answer("", Color::White, Color::Cyan).len(), 1);
}

// 超宽表格：总宽被压到上限内，单元格以 … 截断，但仍是等宽框线。
#[test]
fn render_table_truncates_overwide_cells() {
    let long = "很".repeat(80);
    let rows = vec![
        vec!["列A".to_string(), "B".to_string()],
        vec![long, "x".to_string()],
    ];
    let lines = render_table(&rows, Color::White);
    for l in &lines {
        let text: String = l.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(
            display_width_pub(&text) <= MAX_TABLE_WIDTH,
            "超出上限: {text}"
        );
    }
    let data: String = lines[3].spans.iter().map(|s| s.content.as_ref()).collect();
    assert!(data.contains('…'), "被截断的单元格应有省略号: {data}");
}

// 回归：模型输出前导空行（如 "\n\n你好"）不应在界面上产生空行。
#[test]
fn flush_answer_trims_leading_blank_lines() {
    let mut app = App::default();
    app.push(Role::User, "❯ hi".to_string());
    app.on_event(AgentEvent::Token("\n\n你好！".to_string()));
    app.on_event(AgentEvent::Finished);

    let texts: Vec<String> = app.scrollback.iter().map(|r| r.text()).collect();
    // ❯ hi → 回答 → 完成标记；中间不允许出现空行。
    assert_eq!(
        texts,
        vec![
            "❯ hi".to_string(),
            "你好！".to_string(),
            "— 完成 —".to_string()
        ],
        "{texts:?}"
    );
    assert_eq!(app.last_answer, "你好！", "剪贴板内容同样裁剪");
}

// Markdown 轻渲染：标题加粗去井号、分隔线变横线、行内粗体/代码解析、列表符替换。
#[test]
fn format_answer_renders_inline_markdown() {
    let md = "# 标题一\n---\n- 列表项 **重点** 与 `code` 混排\n普通 **粗体** 行";
    let rows = format_answer(md, Color::White, Color::Cyan);

    let h: String = rows[0].text();
    assert_eq!(h, "标题一", "标题应去掉井号");
    match &rows[0] {
        Row::Styled(l) => {
            let s = &l.spans[0];
            assert!(s.style.add_modifier.contains(Modifier::BOLD), "标题应加粗");
        }
        _ => panic!("标题应为 Styled"),
    }

    // 分隔线：Verbatim 横线。
    match &rows[1] {
        Row::Verbatim(l) => {
            let t: String = l.spans.iter().map(|s| s.content.as_ref()).collect();
            assert!(t.starts_with('─') && t.chars().all(|c| c == '─'));
        }
        _ => panic!("分隔线应为 Verbatim"),
    }

    // 列表行：• 替换 + 行内样式段存在（粗体与代码颜色不同）。
    match &rows[2] {
        Row::Styled(l) => {
            let full: String = l.spans.iter().map(|s| s.content.as_ref()).collect();
            assert_eq!(full, "• 列表项 重点 与 code 混排");
            assert!(l.spans.len() >= 4, "应有多个样式分段: {:?}", l.spans);
        }
        _ => panic!("列表行应为 Styled"),
    }
}

// /examples：Document 事件按 Markdown 渲染进历史。
#[test]
fn document_event_renders_markdown_rows() {
    let mut app = App::default();
    app.on_event(AgentEvent::Document(
        "# 使用示例\n| a | b |\n|---|\n| 1 | 2 |".into(),
    ));
    let texts: Vec<String> = app.scrollback.iter().map(|r| r.text()).collect();
    assert_eq!(texts[0], "使用示例", "标题去井号: {texts:?}");
    assert!(texts.iter().any(|t| t.starts_with('┌')), "表格应渲染成框线");
}

// 续接会话：Capabilities 事件应更新常驻能力摘要。
#[test]
fn capabilities_event_updates_caps() {
    let mut app = App::default();
    assert!(app.caps.is_none());
    app.on_event(AgentEvent::Capabilities {
        skills: 1,
        tools: 5,
        mcp_online: 1,
    });
    let c = app.caps.expect("caps 应被设置");
    assert_eq!((c.skills, c.tools, c.mcp_online), (1, 5, 1));
}

// 续接会话：Resumed 事件应把恢复的历史回放到可见记录，避免续接后屏幕空白。
#[test]
fn resumed_event_replays_history_into_scrollback() {
    let mut app = App::default();
    let items = vec![
        InputItem::message("user", "hi"),
        InputItem::message("assistant", "你好，有什么可以帮您？"),
        InputItem::FunctionCall {
            call_id: "c1".into(),
            name: "shell".into(),
            arguments: "{\"command\":\"echo ok\"}".to_string(),
            id: "fc_c1".into(),
        },
        InputItem::function_call_output("c1".into(), "ok".into()),
    ];
    app.on_event(AgentEvent::Resumed(items));

    let rendered: Vec<String> = app.scrollback.iter().map(|r| r.text()).collect();
    assert!(
        rendered.iter().any(|t| t.contains("❯ hi")),
        "用户消息应回放: {rendered:?}"
    );
    assert!(
        rendered
            .iter()
            .any(|t| t.contains("你好，有什么可以帮您？")),
        "助手消息应回放: {rendered:?}"
    );
    assert!(
        rendered.iter().any(|t| t.contains("→ 调用 shell")),
        "工具调用应回放: {rendered:?}"
    );
    assert!(
        rendered.iter().any(|t| t.contains("↳ ok")),
        "工具结果应回放: {rendered:?}"
    );
    assert_eq!(app.scroll_offset, 0, "续接后应定位到最新（底部）");
}
