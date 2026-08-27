//! 纯文本格式化工具：行内 Markdown、GFM 表格框线、能力面摘要、显示宽度计算。
//!
//! 这些函数不依赖 [`App`](crate::tui::app::App) 状态，便于独立测试与复用。

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

use crate::skills::Skill;

/// 行内 Markdown：`**粗体**` 与 `` `代码` `` 解析为带样式的 Span 序列。
pub(crate) fn parse_inline(text: &str, base: Color, code_color: Color) -> Line<'static> {
    let base_st = Style::default().fg(base);
    let bold_st = base_st.add_modifier(Modifier::BOLD);
    let code_st = Style::default().fg(code_color).add_modifier(Modifier::BOLD);

    #[derive(PartialEq, Clone, Copy)]
    enum Mode {
        Plain,
        Bold,
        Code,
    }
    let mut mode = Mode::Plain;
    let mut chars: Vec<(char, Style)> = Vec::new();
    let mut it = text.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '*' if it.peek() == Some(&'*') => {
                it.next();
                mode = if mode == Mode::Bold {
                    Mode::Plain
                } else {
                    Mode::Bold
                };
            }
            '`' => {
                mode = if mode == Mode::Code {
                    Mode::Plain
                } else {
                    Mode::Code
                }
            }
            c => {
                let st = match mode {
                    Mode::Plain => base_st,
                    Mode::Bold => bold_st,
                    Mode::Code => code_st,
                };
                chars.push((c, st));
            }
        }
    }

    // 相同样式连续段合并为 Span。
    let mut spans: Vec<Span> = Vec::new();
    let mut buf = String::new();
    let mut cur: Option<Style> = None;
    for (c, st) in chars {
        match cur {
            Some(prev) if prev == st => buf.push(c),
            prev => {
                if let Some(p) = prev {
                    spans.push(Span::styled(std::mem::take(&mut buf), p));
                }
                cur = Some(st);
                buf.push(c);
            }
        }
    }
    if !buf.is_empty()
        && let Some(st) = cur
    {
        spans.push(Span::styled(buf, st));
    }
    Line::from(spans)
}

/// 把单元格矩阵渲染成 Unicode 框线表格；列宽取内容最大显示宽，整体超限时截断单元格。
pub(crate) fn render_table(rows: &[Vec<String>], color: Color) -> Vec<Line<'static>> {
    if rows.is_empty() {
        return vec![];
    }
    let n_cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    // 每列宽 = 所有行该列最大显示宽；预留边框与竖线空间。
    let mut widths = vec![0usize; n_cols];
    for r in rows {
        for (ci, cell) in r.iter().enumerate() {
            widths[ci] = widths[ci].max(display_width_pub(cell));
        }
    }
    // 总宽超限则从最宽列开始削，直到放得下。
    loop {
        let total: usize = widths.iter().sum::<usize>() + n_cols * 3 + 1;
        if total <= crate::tui::app::MAX_TABLE_WIDTH || widths.iter().all(|w| *w <= 1) {
            break;
        }
        let max_i = widths
            .iter()
            .enumerate()
            .max_by_key(|(_, w)| **w)
            .map(|(i, _)| i)
            .unwrap_or(0);
        widths[max_i] -= 1;
    }

    let cut = |s: &str, w: usize| -> String {
        if display_width_pub(s) <= w {
            return s.to_string();
        }
        // 按显示宽截断并加省略号。
        let mut out = String::new();
        let mut used = 0usize;
        for c in s.chars() {
            let cw = if (c as u32) >= 0x2E80 { 2 } else { 1 } as usize;
            if used + cw > w.saturating_sub(2) {
                break;
            }
            out.push(c);
            used += cw;
        }
        while used < w.saturating_sub(2) {
            out.push(' ');
            used += 1;
        }
        format!("{out}…")
    };

    let pad_cell = |s: &str, w: usize| -> String {
        let mut out = cut(s, w);
        let dw = display_width_pub(&out).min(w);
        for _ in dw..w {
            out.push(' ');
        }
        out
    };
    let border = |l: &str, m: &str, r: &str| -> String {
        let mut s = String::from(l);
        for (i, w) in widths.iter().enumerate() {
            if i > 0 {
                s.push_str(m);
            }
            s.push_str(&"─".repeat(w + 2));
        }
        s.push_str(r);
        s
    };

    let mk_line = |content: String| -> Line<'static> {
        Line::from(Span::styled(content, Style::default().fg(color)))
    };

    let mut out = Vec::new();
    let mut first_data = true;
    for (ri, r) in rows.iter().enumerate() {
        if ri == 0 {
            out.push(mk_line(border("┌", "┬", "┐")));
        } else if first_data {
            out.push(mk_line(border("├", "┼", "┤")));
            first_data = false;
        }
        let mut line = String::from("│");
        for (ci, w) in widths.iter().enumerate() {
            let cell = r.get(ci).map(String::as_str).unwrap_or("");
            line.push(' ');
            line.push_str(&pad_cell(cell, *w));
            line.push_str(" │");
        }
        out.push(mk_line(line));
        if ri == rows.len() - 1 {
            out.push(mk_line(border("└", "┴", "┘")));
        }
    }
    out
}

/// 显示宽度（CJK/全角算 2 列）。与 render.rs 的实现保持一致语义。
pub(crate) fn display_width_pub(s: &str) -> usize {
    s.chars()
        .map(|c| if (c as u32) >= 0x2E80 { 2 } else { 1 })
        .sum()
}

/// 能力明细：概览一行 + 两空格缩进的分类明细，保证启动日志左缘对齐、层级清晰。
/// 所有行都经 `AgentEvent::System` 输出（统一带 `[系统]` 前缀），此处不再各自加标签。
pub(crate) fn capability_lines(
    tools: &[(String, String, bool)],
    skills: &[Skill],
    mcp_status: &[String],
) -> Vec<String> {
    let builtin = tools
        .iter()
        .filter(|(n, _, _)| !n.starts_with("mcp_"))
        .count();
    let remote = tools.len() - builtin;
    let online = mcp_status.iter().filter(|l| l.contains("已连接")).count();
    let mut out = vec![format!(
        "能力概览｜工具 {}（内置 {} + 远端 {}）· 技能 {} · MCP 在线 {}；明细随时 /tools /skills /mcp 查看",
        tools.len(),
        builtin,
        remote,
        skills.len(),
        online,
    )];
    for l in mcp_status {
        out.push(format!("  MCP {l}"));
    }
    for s in skills {
        let trig = if s.triggers.is_empty() {
            String::new()
        } else {
            format!("［触发词: {}］", s.triggers.join("/"))
        };
        out.push(format!("  技能 {}：{}{trig}", s.name, s.description));
    }
    out
}
