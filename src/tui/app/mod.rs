//! TUI 状态与事件处理：滚动历史、输入框编辑、agent 事件到界面的映射。
//!
//! 输入框编辑与翻页见 `input` 子模块；测试见 `tests` 子模块。

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

use super::format::{parse_inline, render_table};
use super::render::push_help;
use super::theme::Role;
use super::theme::Theme;
use super::util::{pretty_args, truncate_ellipsis};
use crate::agent::AgentEvent;
use crate::model::InputItem;

mod input;

/// 滚动历史上限：超过后裁掉最旧行，防止长时间会话内存无限增长。
const MAX_SCROLLBACK: usize = 5000;

/// 历史行的三种形态：
/// - `Styled`：普通文本，折行时优先在空格处断词；
/// - `Verbatim`：预排版行（如 Markdown 表格的框线），只做硬折行，
///   绝不吞并/插入空格，否则列对齐会被破坏；
/// - `Log`：带前缀的日志行，折行后续行按 `hang` 列悬挂缩进，
///   保证长日志换行后仍与正文首字对齐（竖直对齐）。
#[derive(Clone, Debug)]
pub(crate) enum Row {
    Styled(Line<'static>),
    Verbatim(Line<'static>),
    Log { line: Line<'static>, hang: u16 },
}

impl Row {
    /// 提取整行纯文本（导出 / 测试用）。
    pub(crate) fn text(&self) -> String {
        let line = match self {
            Row::Styled(l) | Row::Verbatim(l) => l,
            Row::Log { line, .. } => line,
        };
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }
}

/// 表格渲染的最大总宽：超出后截断单元格内容（保留表头完整）。
pub(crate) const MAX_TABLE_WIDTH: usize = 100;

/// 把助手回答文本拆成可显示的行：普通段落原样一行；
/// GFM 表格块（连续 `|…|` 行 + `---` 分隔行）渲染成等宽框线 Verbatim 行。
/// 仅做轻量解析：不处理代码围栏内的表格（罕见场景，接受误判）。
pub(crate) fn format_answer(text: &str, color: Color, accent: Color) -> Vec<Row> {
    let mut out: Vec<Row> = Vec::new();
    let mut i = 0;
    let lines: Vec<&str> = text.lines().collect();

    let is_table_line = |l: &str| l.trim_start().starts_with('|') && l.matches('|').count() >= 2;
    let is_sep_line = |l: &str| {
        l.trim_start().starts_with('|')
            && l.matches('|').count() >= 2
            && l.replace(['|', '-', ':', ' '], "").is_empty()
            && l.contains('-')
    };

    while i < lines.len() {
        if is_table_line(lines[i]) && i + 1 < lines.len() && is_sep_line(lines[i + 1]) {
            // 收集整个表格块。
            let start = i;
            let mut end = i + 1;
            while end < lines.len() && is_table_line(lines[end]) {
                end += 1;
            }
            let mut rows: Vec<Vec<String>> = Vec::new();
            for l in &lines[start..end] {
                if is_sep_line(l) {
                    continue;
                }
                let t = l.trim();
                let t = t.strip_prefix('|').unwrap_or(t);
                let t = t.strip_suffix('|').unwrap_or(t);
                rows.push(t.split('|').map(|c| c.trim().to_string()).collect());
            }
            for line in render_table(&rows, color) {
                out.push(Row::Verbatim(line));
            }
            i = end;
            continue;
        }
        let raw = lines[i];
        // 分隔线：仅由连字符（允许空格）组成且至少一个 -。
        if !raw.is_empty() && raw.replace(['-', ' '], "").is_empty() && raw.contains('-') {
            out.push(Row::Verbatim(Line::from(Span::styled(
                "─".repeat(40),
                Style::default().fg(color),
            ))));
            i += 1;
            continue;
        }
        // 标题：去井号整行加粗（强调色）；列表符替换为 •；其余走行内样式解析。
        let trimmed = raw.trim_start();
        let line = if trimmed.starts_with('#') && trimmed[1..].starts_with(['#', ' ']) {
            let body = trimmed.trim_start_matches('#').trim_start();
            Line::from(Span::styled(
                body.to_string(),
                Style::default().fg(accent).add_modifier(Modifier::BOLD),
            ))
        } else if let Some(rest) = raw.strip_prefix("- ") {
            parse_inline(&format!("• {rest}"), color, accent)
        } else {
            parse_inline(raw, color, accent)
        };
        out.push(Row::Styled(line));
        i += 1;
    }
    if out.is_empty() {
        out.push(Row::Styled(Line::from(Span::styled(
            String::new(),
            Style::default().fg(color),
        ))));
    }
    out
}

/// TUI 界面状态。
#[derive(Default)]
pub(crate) struct App {
    /// 历史行（已按当前主题着色；提示块可含多段高亮，表格为预排版 Verbatim 行）。
    pub(crate) scrollback: Vec<Row>,
    /// 正在流式累积的助手回答（未落盘到 scrollback）。
    pub(crate) answer_buf: String,
    /// 底部输入框内容。
    pub(crate) input: String,
    /// 输入框光标（按字节偏移，始终位于字符边界上）。
    pub(crate) input_cursor: usize,
    /// 状态栏基础信息（model / sandbox）。
    pub(crate) status: String,
    /// 状态栏动态信息（token 用量 / 预算 / 是否触发工具）。
    pub(crate) info: String,
    /// 推理摘要（reasoning 模型才有），逐段累积。
    pub(crate) reasoning: Vec<String>,
    /// 是否展开推理摘要。
    pub(crate) show_reasoning: bool,
    /// 是否有任务正在运行（运行时不接受新提交）。
    pub(crate) running: bool,
    /// 待审批的工具调用：`(id, name, args)`，非空时键盘只接受 y/n。
    pub(crate) pending_approval: Option<(String, String, String)>,
    /// 从底部向上翻看的行数（0 = 跟随最新输出）。
    pub(crate) scroll_offset: usize,
    /// 最近一次渲染的历史区可见行数（用于计算翻页步长）。
    pub(crate) viewport_h: usize,
    /// 请求退出。
    pub(crate) should_quit: bool,
    /// 自增帧计数，用于「正在思考」指示器的动画。
    pub(crate) ticks: u64,
    /// 中途取消信号（与 agent 任务共享）；运行中按 Esc 置位以中止生成。
    pub(crate) cancel: Arc<AtomicBool>,
    /// 当前模型名（与 agent 任务共享，可由 `/model` 运行时切换）。
    pub(crate) model: Arc<Mutex<String>>,
    /// 中间态状态：多 Agent 三角色编排，共享 agent 任务中 `pse`（AtomicBool）的状态。
    pub(crate) pse: Arc<AtomicBool>,
    /// 最近一次完整回答的文本（用于 Ctrl-Y 复制到剪贴板）。
    pub(crate) last_answer: String,
    /// 上一次收到用量事件的时间，用于估算实时生成速率（tok/s）。
    pub(crate) last_usage: Option<std::time::Instant>,
    /// 当前配色主题（暗/亮），影响所有文本颜色解析。
    pub(crate) theme: Theme,
    /// 能力面摘要（技能 / 工具 / MCP），常驻显示在输入框标题栏。
    pub(crate) caps: Option<Caps>,
}

/// 启动时的能力快照（由 agent 任务连接完 MCP / 加载技能后广播）。
#[derive(Clone, Copy, Debug)]
pub(crate) struct Caps {
    pub(crate) skills: usize,
    pub(crate) tools: usize,
    pub(crate) mcp_online: usize,
}

impl App {
    /// 追加一行到历史；超过上限则裁掉最旧行。
    pub(crate) fn push_row(&mut self, row: Row) {
        self.scrollback.push(row);
        if self.scrollback.len() > MAX_SCROLLBACK {
            let drop = self.scrollback.len() - MAX_SCROLLBACK;
            self.scrollback.drain(..drop);
        }
    }

    /// 追加一行已着色的 Line（普通文本形态）。
    pub(crate) fn push_line(&mut self, line: Line<'static>) {
        self.push_row(Row::Styled(line));
    }

    pub(crate) fn push(&mut self, role: Role, text: String) {
        let color = self.theme.color(role);
        self.push_line(Line::from(Span::styled(text, color)));
    }

    /// 把当前可见的对话记录与推理过程导出为 Markdown 文件。
    pub(crate) fn export_markdown(&self, path: &str) -> std::io::Result<()> {
        let mut out = String::new();
        out.push_str("# resolve-tui 会话导出\n\n");
        out.push_str(&format!(
            "- 时间：{}\n",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
        ));
        let model = self.model.lock().unwrap_or_else(|e| e.into_inner()).clone();
        out.push_str(&format!("- 模型：{model}\n"));
        out.push_str(&format!("- 状态：{}\n\n", self.status));

        out.push_str("## 对话记录\n\n");
        for row in &self.scrollback {
            out.push_str(&row.text());
            out.push('\n');
        }

        if !self.reasoning.is_empty() {
            out.push_str("\n## 推理过程\n\n");
            for r in &self.reasoning {
                out.push_str(r);
                out.push('\n');
            }
        }

        std::fs::write(path, out)
    }

    /// 把已完成的助手回答刷入历史。
    fn flush_answer(&mut self) {
        // 首尾空白（部分模型会输出前导换行）不进入展示与剪贴板。
        let text = self.answer_buf.trim().to_string();
        if !text.is_empty() {
            // 记录最近一次完整回答，供 Ctrl-Y 复制。
            self.last_answer = text.clone();
            let color = self.theme.color(Role::Assistant);
            let accent = self.theme.color(Role::ToolCall);
            for row in format_answer(&text, color, accent) {
                self.push_row(row);
            }
        }
        self.answer_buf.clear();
    }

    pub(crate) fn on_event(&mut self, event: AgentEvent) {
        match event {
            AgentEvent::Token(t) => self.answer_buf.push_str(&t),
            AgentEvent::ToolCall { name, id } => {
                self.flush_answer();
                self.push(Role::ToolCall, format!("→ 调用 {name} ({id})"));
            }
            AgentEvent::ToolResult {
                ok, chars, preview, ..
            } => {
                // 失败时把原因直接亮出来，不用去猜。
                let suffix = preview
                    .as_ref()
                    .map(|p| format!("：{p}"))
                    .unwrap_or_default();
                self.push(
                    if ok { Role::ToolResult } else { Role::Error },
                    format!(
                        "  ↳ {}  ({} chars){suffix}",
                        if ok { "ok" } else { "err" },
                        chars
                    ),
                );
            }
            AgentEvent::Reasoning(r) => self.reasoning.push(r),
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
                // 估算本轮生成速率：相邻 Usage 事件的时间差 ≈ 单轮耗时。
                let speed = self.last_usage.and_then(|t0| {
                    let dt = t0.elapsed().as_secs_f64();
                    if dt > 0.0 {
                        Some(output_tokens as f64 / dt)
                    } else {
                        None
                    }
                });
                self.last_usage = Some(std::time::Instant::now());
                let speed_str = speed.map(|s| format!(" {s:.1} tok/s")).unwrap_or_default();
                self.info = format!(
                    "tok in={input_tokens} out={output_tokens} tools={} budget={budget}{speed_str}",
                    if had_tools { "Y" } else { "N" }
                );
            }
            AgentEvent::ToolApproval { id, name, args } => {
                self.flush_answer();
                self.push(
                    Role::Approval,
                    format!("! 待确认 {}", pretty_args(&name, &args)),
                );
                self.pending_approval = Some((id, name, args));
            }
            AgentEvent::System(s) => {
                // 日志行：折行后续行悬挂缩进到「本行正文起点」，保持竖直对齐。
                // 前缀宽 = "[系统] "(7 列) + 消息自带的前导空格（明细行的层级缩进）。
                let lead = s.len() - s.trim_start().len();
                let color = self.theme.color(Role::System);
                let line = Line::from(Span::styled(format!("[系统] {s}"), color));
                self.push_row(Row::Log {
                    line,
                    // "[" 1 + "系统" 4 + "]" 1 + 空格 1 = 7，再加消息自身前导空格。
                    hang: 7 + lead as u16,
                });
            }
            AgentEvent::Error(m) => self.push(Role::Error, format!("✗ {m}")),
            AgentEvent::Iteration(_) => {}
            AgentEvent::Document(text) => {
                // 文档（如 /examples）：按 Markdown 样式渲染进历史。
                let color = self.theme.color(Role::Help);
                let accent = self.theme.color(Role::ToolCall);
                for row in format_answer(&text, color, accent) {
                    self.push_row(row);
                }
            }
            AgentEvent::Capabilities {
                skills,
                tools,
                mcp_online,
            } => {
                self.caps = Some(Caps {
                    skills,
                    tools,
                    mcp_online,
                });
            }
            AgentEvent::ToggleReasoning => {
                self.show_reasoning = !self.show_reasoning;
                self.push(
                    Role::Hint,
                    format!(
                        "[推理] 已{}",
                        if self.show_reasoning {
                            "展开"
                        } else {
                            "折叠"
                        }
                    ),
                );
            }
            AgentEvent::ClearScreen => {
                self.scrollback.clear();
                self.reasoning.clear();
                self.answer_buf.clear();
                self.scroll_offset = 0;
                // 清空对话后仍保留帮助提示，避免按键/命令说明丢失。
                push_help(self);
            }
            AgentEvent::Export(path) => match self.export_markdown(&path) {
                Ok(_) => self.push(Role::System, format!("[导出] 已写入 {path}")),
                Err(e) => self.push(Role::Error, format!("✗ 导出失败：{e}")),
            },
            AgentEvent::Finished => {
                self.flush_answer();
                self.running = false;
                self.push(Role::Hint, "— 完成 —".to_string());
            }
            AgentEvent::Resumed(items) => self.replay_resumed(&items),
            AgentEvent::Quit => {
                self.should_quit = true;
            }
        }
    }

    /// 把续接/载入的会话历史回放到可见记录，避免续接后屏幕空白、误以为会话丢失。
    /// 仅展示用：角色着色与运行期事件保持一致，工具调用/结果做截断以免刷屏。
    fn replay_resumed(&mut self, items: &[InputItem]) {
        for item in items {
            match item {
                InputItem::Message { role, content } => {
                    let text: String = content
                        .iter()
                        .map(|c| c.text.as_str())
                        .collect::<Vec<_>>()
                        .join("\n");
                    match role.as_str() {
                        "user" => self.push(Role::User, format!("❯ {text}")),
                        "assistant" => self.push(Role::Assistant, text),
                        _ => self.push(Role::System, text),
                    }
                }
                InputItem::FunctionCall {
                    name, arguments, ..
                } => {
                    // 按字符截断：字节切片会切进多字节中文中间直接 panic。
                    let args = truncate_ellipsis(&arguments.to_string(), 200);
                    self.push(Role::ToolCall, format!("→ 调用 {name} {args}"));
                }
                InputItem::FunctionCallOutput { output, .. } => {
                    let out = truncate_ellipsis(output, 400);
                    self.push(Role::ToolResult, format!("↳ {out}"));
                }
            }
        }
        // 续接后定位到最新（底部），与正常运行时的跟随行为一致。
        self.scroll_offset = 0;
    }
}

#[cfg(test)]
mod tests;
