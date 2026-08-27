//! 配色主题：语义角色 → 具体颜色的映射，含亮/暗两套与终端背景自动探测（OSC 11）。

use std::io::IsTerminal;
use std::time::Duration;

use ratatui::style::Color;
use tokio::io::{AsyncReadExt, stdin as tokio_stdin};

/// 语义角色：不同内容类型映射到主题中的具体颜色，便于切换亮/暗主题。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Role {
    User,
    Assistant,
    System,
    Error,
    ToolCall,
    ToolResult,
    Reasoning,
    Help,
    Approval,
    Hint,
}

/// 配色方案：暗色（黑底）与亮色（白底）两套，避免在浅色终端下文字看不清。
#[derive(Clone, Debug)]
pub(crate) struct Theme {
    user: Color,
    assistant: Color,
    system: Color,
    error: Color,
    tool_call: Color,
    tool_result: Color,
    reasoning: Color,
    help: Color,
    approval: Color,
    hint: Color,
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark()
    }
}

impl Theme {
    pub(crate) fn dark() -> Self {
        Self {
            user: Color::Green,
            assistant: Color::White,
            system: Color::DarkGray,
            error: Color::Red,
            tool_call: Color::Cyan,
            tool_result: Color::DarkGray,
            reasoning: Color::DarkGray,
            help: Color::DarkGray,
            approval: Color::Yellow,
            hint: Color::DarkGray,
        }
    }

    /// 亮色（白底）主题：用深色文字保证对比度，工具调用改用蓝、审批改用品红。
    pub(crate) fn light() -> Self {
        Self {
            user: Color::Green,
            assistant: Color::Black,
            system: Color::DarkGray,
            error: Color::Red,
            tool_call: Color::Blue,
            tool_result: Color::DarkGray,
            reasoning: Color::DarkGray,
            help: Color::DarkGray,
            approval: Color::Magenta,
            hint: Color::DarkGray,
        }
    }

    pub(crate) fn color(&self, role: Role) -> Color {
        match role {
            Role::User => self.user,
            Role::Assistant => self.assistant,
            Role::System => self.system,
            Role::Error => self.error,
            Role::ToolCall => self.tool_call,
            Role::ToolResult => self.tool_result,
            Role::Reasoning => self.reasoning,
            Role::Help => self.help,
            Role::Approval => self.approval,
            Role::Hint => self.hint,
        }
    }
}

/// 通过 OSC 11 查询终端背景色，自动选择亮/暗主题。
/// 仅当 stdin/stdout 均为 TTY 时探测；非 TTY、终端不支持或超时均回退暗色。
/// 注：调用前 run_tui 已 `enable_raw_mode`，stdin 处于非规范模式，回包字节可立即读到。
pub(crate) async fn detect_is_light_bg() -> bool {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return false;
    }
    // 发出背景色查询（OSC 11），以 ST（ESC \）结尾。
    {
        use std::io::Write;
        let mut out = std::io::stdout();
        if out.write_all(b"\x1b]11;?\x1b\\").is_err() {
            return false;
        }
        let _ = out.flush();
    }

    let mut s = tokio_stdin();
    let mut buf = [0u8; 1];
    let mut resp = Vec::with_capacity(32);
    let start = tokio::time::Instant::now();
    let timeout = Duration::from_millis(150);
    loop {
        let elapsed = start.elapsed();
        if elapsed >= timeout {
            break;
        }
        match tokio::time::timeout(timeout.saturating_sub(elapsed), s.read(&mut buf)).await {
            // 结束符：BEL(0x07) 或 ST(ESC \)
            Ok(Ok(0)) => break, // EOF
            Ok(Ok(_)) => {
                resp.push(buf[0]);
                if resp.len() > 64
                    || buf[0] == 0x07
                    || (resp.len() >= 2
                        && resp[resp.len() - 2] == 0x1b
                        && resp[resp.len() - 1] == b'\\')
                {
                    break;
                }
            }
            Ok(Err(_)) | Err(_) => break, // 读错误 / 超时
        }
    }
    parse_osc11(&resp).unwrap_or(false)
}

/// 解析 OSC 11 回包中的 `rgb:RRRR/GGGG/BBBB`（8/16 位每通道均可），返回背景是否偏亮。
fn parse_osc11(resp: &[u8]) -> Option<bool> {
    let s = String::from_utf8_lossy(resp);
    let rgb = s.find("rgb:").map(|i| &s[i + 4..])?;
    let mut parts = rgb.split('/');
    let to255 = |h: &str| -> Option<u8> {
        // 16 位分量取高字节，8 位分量直接用。
        let h = if h.len() > 2 { &h[..2] } else { h };
        u8::from_str_radix(h, 16).ok()
    };
    let r = to255(parts.next()?)?;
    let g = to255(parts.next()?)?;
    let b = to255(parts.next()?)?;
    // 相对亮度（0–255），>128 视为浅色背景。
    let lum = (r as u32 * 299 + g as u32 * 587 + b as u32 * 114) / 1000;
    Some(lum > 128)
}

#[cfg(test)]
mod tests {
    use super::*;

    // OSC 11 解析：正确区分亮/暗背景，兼容 8/16 位分量，无法解析时返回 None。
    #[test]
    fn parse_osc11_detects_light_and_dark() {
        assert_eq!(parse_osc11(b"\x1b]11;rgb:0000/0000/0000\x07"), Some(false));
        assert_eq!(parse_osc11(b"\x1b]11;rgb:ffff/ffff/ffff\x1b\\"), Some(true));
        assert_eq!(parse_osc11(b"\x1b]11;rgb:ff/ff/ff\x07"), Some(true));
        // 浅灰（亮度 ~ 0.78）仍判为亮色背景。
        assert_eq!(parse_osc11(b"\x1b]11;rgb:c8c8/c8c8/c8c8\x07"), Some(true));
        assert_eq!(parse_osc11(b"garbage"), None);
    }

    // 主题：亮色（白底）下助手文字应为深色（黑），与暗色（白）区分，保证可读。
    #[test]
    fn light_theme_uses_dark_text_on_white_bg() {
        assert_eq!(Theme::dark().color(Role::Assistant), Color::White);
        assert_eq!(Theme::light().color(Role::Assistant), Color::Black);
        assert_eq!(Theme::light().color(Role::ToolCall), Color::Blue);
    }
}
