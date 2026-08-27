//! 渲染：整体布局、帮助提示块、按显示宽度折行（保留多段样式）。

use std::sync::atomic::Ordering;

use crate::tui::wrap::*;
use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, Paragraph},
};

use super::app::App;
use super::theme::{Role, Theme};

/// 按当前主题把提示模板渲染成带高亮的 Line。
/// 模板中用反引号 `...` 包裹的内容使用强调色（命令/按键），其余用帮助色。
fn styled_hint(theme: &Theme, tmpl: &str) -> Line<'static> {
    let accent = theme.color(Role::ToolCall);
    let dim = theme.color(Role::Help);
    let mut spans: Vec<Span> = Vec::new();
    for (i, part) in tmpl.split('`').enumerate() {
        if part.is_empty() {
            continue;
        }
        let style = if i % 2 == 1 {
            Style::default().fg(accent)
        } else {
            Style::default().fg(dim)
        };
        spans.push(Span::styled(part.to_string(), style));
    }
    Line::from(spans)
}

/// 把结构化的使用提示（带命令/按键高亮）分行推入历史；清空对话后仍保留。
pub(crate) fn push_help(app: &mut App) {
    let theme = app.theme.clone();
    app.push_line(Line::from(Span::styled(
        "—— 使用提示 ——",
        Style::default()
            .fg(theme.color(Role::ToolCall))
            .add_modifier(Modifier::BOLD),
    )));
    for tmpl in [
        "输入后按 `Enter` 提交；`Esc` 退出（运行中按 `Esc` 中止生成）",
        "`PageUp` / `PageDown` 翻看历史；`Ctrl-R`（或 `/reasoning`）切换推理展示",
        "`Ctrl-Y` 复制最近一次回答；`/export` 导出；`/examples` 示例",
        "`/model` [模型名] 切换模型；`/tools` [on|off 名] 启停工具",
        "`/skills` `/mcp add|remove` 管理技能与 MCP；`/help` 查看全部命令",
        "会话管理：`/list` `/create` `/apply` `/save` `/load` `/clear` `/rm`",
        "退出自动存档，启动自动续接",
    ] {
        app.push_line(styled_hint(&theme, tmpl));
    }
}

/// 渲染整个界面：上方面板为滚动历史（支持 PageUp/PageDown 翻看），下方为输入行。
pub(crate) fn ui(f: &mut Frame, app: &mut App) {
    let chunks = Layout::vertical([Constraint::Min(1), Constraint::Length(3)]).split(f.area());

    // 记录视口高度供翻页步长使用；去掉上下边框后的可见行数。
    let visible = (chunks[0].height.saturating_sub(2) as usize).max(1);
    app.viewport_h = visible;
    // 面板内部可用宽度（去掉左右边框），用于长行折行。
    let inner_w = chunks[0].width.saturating_sub(2);

    // 先收集要显示的行（含多段高亮），再统一按面板宽度折行，
    // 避免长行（如启动帮助提示）被 List 截断而看不到后半截。
    let mut items: Vec<Line<'static>> = Vec::new();
    // 历史：普通行按空格断词折行；Verbatim（表格框线）硬折行；日志行折行后悬挂缩进。
    for row in &app.scrollback {
        match row {
            crate::tui::app::Row::Styled(line) => items.extend(wrap_line_spans(line, inner_w)),
            crate::tui::app::Row::Verbatim(line) => items.extend(hard_wrap_spans(line, inner_w)),
            crate::tui::app::Row::Log { line, hang } => {
                items.extend(wrap_line_hanging(line, inner_w, *hang))
            }
        }
    }
    if !app.answer_buf.is_empty() {
        let color = app.theme.color(Role::Assistant);
        for row in wrap_line(&app.answer_buf, inner_w) {
            items.push(Line::from(Span::styled(row, Style::default().fg(color))));
        }
    }
    // 推理摘要：折叠时只显示一行提示，按 Ctrl-R 展开。
    if !app.reasoning.is_empty() {
        let color = app.theme.color(Role::Reasoning);
        if app.show_reasoning {
            for r in &app.reasoning {
                for row in wrap_line(r, inner_w) {
                    items.push(Line::from(Span::styled(row, Style::default().fg(color))));
                }
            }
        } else {
            for row in wrap_line(
                &format!(
                    "[推理过程 {} 行 · 按 Ctrl-R 或 /reasoning 展开]",
                    app.reasoning.len()
                ),
                inner_w,
            ) {
                items.push(Line::from(Span::styled(row, Style::default().fg(color))));
            }
        }
    }
    // 运行中且尚无任何可见输出（推理/回答）时，给出「正在思考」提示，
    // 避免请求往返或模型冷启动期间界面像卡死。带一个转圈动画表明仍在活动。
    if app.running && app.answer_buf.is_empty() && app.reasoning.is_empty() {
        const SPIN: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
        let spin = SPIN[(app.ticks / 2) as usize % SPIN.len()];
        let color = app.theme.color(Role::Hint);
        for row in wrap_line(&format!("{spin} 正在思考…"), inner_w) {
            items.push(Line::from(Span::styled(row, Style::default().fg(color))));
        }
    }

    // 按偏移取窗口：offset=0 跟随最新，向上翻则往前切一屏。
    let total = items.len();
    let end = total.saturating_sub(app.scroll_offset.min(total));
    let start = end.saturating_sub(visible);
    items.drain(..start);
    items.truncate(end - start);

    let mut title = String::from("resolve-tui");
    if app.scroll_offset > 0 {
        title.push_str(&format!(
            " · 已上翻 {} 行（PageDown 回到底部）",
            app.scroll_offset
        ));
    }
    let list_widget = List::new(items).block(Block::default().borders(Borders::ALL).title(title));
    f.render_widget(list_widget, chunks[0]);

    let prompt = Span::styled(
        "❯ ",
        Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD),
    );
    let model = app.model.lock().unwrap_or_else(|e| e.into_inner()).clone();
    // 能力面常驻摘要。刻意不用 emoji：其渲染宽度在不同终端不一致，
    // 会把 Block 标题与右边框挤错位；中文/数字的列宽是确定的。
    let caps_str = match app.caps {
        Some(c) => format!("｜工具{} 技能{} MCP{}", c.tools, c.skills, c.mcp_online),
        None => String::new(),
    };
    let title = if app.pending_approval.is_some() {
        "! 工具审批：[y 允许 / n 拒绝]".to_string()
    } else {
        format!(
            "model={}{} {} pse={} | {} | {}",
            model,
            caps_str,
            app.status,
            if app.pse.load(Ordering::SeqCst) {
                "on"
            } else {
                "off"
            },
            app.info,
            if app.running {
                "● 运行中…"
            } else {
                "就绪 · Enter 发送 · Esc 退出"
            }
        )
    };
    let input = Paragraph::new(Line::from(vec![prompt, Span::raw(app.input.clone())]))
        .block(Block::default().borders(Borders::ALL).title(title));
    f.render_widget(input, chunks[1]);

    // 把光标定位到输入框内光标处：raw 模式下 IME 的拼音/候选预编辑串会画在终端光标处，
    // 若不显式定位，每次重绘后光标回到左上角，中文输入就会「往上漂」。运行中则隐藏光标。
    if !app.running {
        let before_cursor = &app.input[..app.input_cursor];
        let x = chunks[1].x + 1 + 2 + display_width(before_cursor);
        let y = chunks[1].y + 1;
        f.set_cursor_position((x, y));
    }
}

/// 近似显示宽度：CJK / 全角字符算 2 列，其余 1 列（仅用于定位光标）。
#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentEvent;

    // 渲染：回答刷入历史后，在给定视口内应当可见。
    #[test]
    fn renders_answer_in_history_viewport() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut app = App::default();
        app.on_event(AgentEvent::Token("Hi there!".to_string()));
        app.on_event(AgentEvent::Finished);

        let backend = TestBackend::new(80, 12);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| ui(f, &mut app)).unwrap();
        let text: String = term
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(
            text.contains("Hi there!"),
            "回答应当在视口内可见，实际渲染:\n{text}"
        );
    }

    // 日志行悬挂缩进：折行后续行左侧补 hang 列空白，与首行正文竖直对齐。
    #[test]
    fn wrap_line_hanging_indents_continuation_lines() {
        let line = Line::from(Span::styled(
            "[系统] 能力概览｜工具 19（内置 4 + 远端 15）· 技能 1 · MCP 在线 2",
            Style::default(),
        ));
        let rows = wrap_line_hanging(&line, 30, 7);
        assert!(rows.len() >= 2, "窄面板下应折成多行: {rows:?}");
        for (i, r) in rows.iter().enumerate() {
            let text: String = r.spans.iter().map(|s| s.content.as_ref()).collect();
            let w: usize = text
                .chars()
                .map(|c| if (c as u32) >= 0x2E80 { 2 } else { 1 })
                .sum();
            assert!(w <= 30, "每行不超宽: {text:?}");
            if i > 0 {
                assert!(text.starts_with("       "), "续行应悬挂缩进 7 列: {text:?}");
                // 续行正文起点 = 首行 "[系统] " 之后的位置。
                assert!(
                    !text[7..].starts_with(' '),
                    "缩进后不应再有多余空格: {text:?}"
                );
            }
        }
        // 拼回的正文（去掉缩进）应包含全部原文内容。
        let joined: String = rows
            .iter()
            .map(|r| {
                r.spans
                    .iter()
                    .map(|s| s.content.as_ref().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("");
        assert!(joined.contains("MCP 在线 2"));
    }

    #[test]
    fn styled_hint_highlights_backticked_tokens() {
        let theme = Theme::light();
        let line = styled_hint(&theme, "按 `Enter` 提交；`Esc` 退出");
        // 反引号包裹的 Enter / Esc 应使用强调色（ToolCall），其余用帮助色。
        let accent = theme.color(Role::ToolCall);
        let dim = theme.color(Role::Help);
        let spans: Vec<(String, Color)> = line
            .spans
            .iter()
            .map(|s| (s.content.as_ref().to_string(), s.style.fg.unwrap()))
            .collect();
        assert_eq!(spans[0].1, dim, "普通文本应使用帮助色");
        assert_eq!(spans[1].0, "Enter", "反引号内容应被提取为高亮段");
        assert_eq!(spans[1].1, accent, "命令/按键应使用强调色");
        assert_eq!(spans[2].1, dim);
        assert_eq!(spans[3].0, "Esc");
        assert_eq!(spans[3].1, accent);
    }

    #[test]
    fn wrap_line_spans_preserves_styles_across_wrap() {
        let theme = Theme::light();
        let line = styled_hint(
            &theme,
            "会话管理：`/list` `/create` `/apply` `/save` `/load` `/clear` `/rm`",
        );

        // 宽度充足时不折行，文字（含空格）应原样保留。
        let single = wrap_line_spans(&line, 200);
        assert_eq!(single.len(), 1, "宽度足够时不应折行");
        let full: String = single[0]
            .spans
            .iter()
            .map(|s| s.content.as_ref().to_string())
            .collect();
        assert_eq!(
            full,
            "会话管理：/list /create /apply /save /load /clear /rm"
        );
        // 样式分段：命令部分应为强调色。
        let accent = theme.color(Role::ToolCall);
        assert!(
            single[0]
                .spans
                .iter()
                .any(|s| s.style.fg == Some(accent) && s.content.as_ref() == "/list"),
            "/list 应保持强调色"
        );

        // 窄面板折行：每行不超宽，且忽略空格后内容完整（换行处吃掉空格属正常折行行为）。
        let rows = wrap_line_spans(&line, 12);
        let squashed: String = rows
            .iter()
            .flat_map(|r| r.spans.iter().map(|s| s.content.as_ref().to_string()))
            .collect::<String>()
            .replace(' ', "");
        assert_eq!(
            squashed, "会话管理：/list/create/apply/save/load/clear/rm",
            "折行不应丢失或改动非空白字符"
        );
        for r in &rows {
            let w: usize = r
                .spans
                .iter()
                .map(|s| display_width(s.content.as_ref()) as usize)
                .sum();
            assert!(w <= 12, "折行后每行宽度应 <= 12，实际 {w}: {r:?}");
        }
    }

    // 长行折行：应在给定显示宽度内断开，且内容完整不丢字。
    #[test]
    fn wrap_line_breaks_long_text_within_width() {
        let text = "输入任务后按 Enter 提交；PageUp/PageDown 翻历史；/reasoning 切换推理";
        let rows = wrap_line(text, 10);
        for r in &rows {
            assert!(
                display_width(r) <= 10,
                "每行显示宽度应 <= 10，实际 {} : {:?}",
                display_width(r),
                r
            );
        }
        // 折行不应替换或丢失原文字（重新拼接后仍是原串的超集/等价）。
        let joined: String = rows.concat();
        for ch in ["输入任务后按", "Enter", "提交", "翻历史", "reasoning"] {
            assert!(joined.contains(ch), "折行后缺失片段 {ch}: {joined}");
        }
    }
}
