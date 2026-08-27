//! 输入框编辑与翻页：光标按字符边界移动（兼容中文），翻页步长自适应视口。
//!
//! 这些方法只操作 `App` 的 `input` / `scroll_*` 字段，与事件映射（`on_event`）
//! 解耦，故单独成模块保持 `mod.rs` 聚焦于「状态 + 事件」。

use crate::tui::app::App;

impl App {
    /// 翻页步长：约半个视口高，至少 1 行、至多 50 行。
    fn scroll_step(&self) -> usize {
        (self.viewport_h / 2).clamp(1, 50)
    }

    pub(crate) fn scroll_up(&mut self) {
        self.scroll_offset += self.scroll_step();
        // 粗略钳制到历史顶部即可；ui() 里的 saturating_sub 会兜住越界。
        self.scroll_offset = self
            .scroll_offset
            .min(self.scrollback.len().saturating_sub(1));
    }

    pub(crate) fn scroll_down(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_sub(self.scroll_step());
    }

    // ---- 输入框编辑：光标按字符边界移动，兼容中文 ----

    pub(crate) fn input_push(&mut self, c: char) {
        self.input.insert(self.input_cursor, c);
        self.input_cursor += c.len_utf8();
    }

    /// 粘贴文本：换行折叠为单空格（CRLF 不重复），其余控制字符丢弃。
    pub(crate) fn input_paste(&mut self, text: &str) {
        let mut last_was_space = false;
        for c in text.chars() {
            if c == '\n' || c == '\r' {
                if !last_was_space {
                    self.input_push(' ');
                    last_was_space = true;
                }
            } else if !c.is_control() {
                self.input_push(c);
                last_was_space = false;
            }
        }
    }

    pub(crate) fn input_backspace(&mut self) {
        if let Some(prev) = self.input[..self.input_cursor].chars().next_back() {
            self.input_cursor -= prev.len_utf8();
            self.input.remove(self.input_cursor);
        }
    }

    pub(crate) fn input_delete(&mut self) {
        if self.input_cursor < self.input.len() {
            self.input.remove(self.input_cursor);
        }
    }

    pub(crate) fn input_left(&mut self) {
        if let Some(prev) = self.input[..self.input_cursor].chars().next_back() {
            self.input_cursor -= prev.len_utf8();
        }
    }

    pub(crate) fn input_right(&mut self) {
        if let Some(next) = self.input[self.input_cursor..].chars().next() {
            self.input_cursor += next.len_utf8();
        }
    }

    pub(crate) fn input_home(&mut self) {
        self.input_cursor = 0;
    }

    pub(crate) fn input_end(&mut self) {
        self.input_cursor = self.input.len();
    }

    pub(crate) fn input_clear_line(&mut self) {
        self.input.clear();
        self.input_cursor = 0;
    }
}
