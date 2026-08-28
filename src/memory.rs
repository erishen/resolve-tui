//! 项目上下文与长期记忆。
//!
//! - `AGENT.md`（工作目录根）：项目说明，随每轮注入 system prompt——让 agent
//!   了解当前项目的约定与偏好，无需每次口头重复；
//! - `MEMORY.md`（`<config_dir>/resolve-tui/`）：跨会话长期记忆，由 TUI 的
//!   `/remember <事实>` 追加，同样随每轮注入。
//!
//! 两者都有单文件长度上限：超长截断头部，避免吃掉过多 token 预算。

use std::path::{Path, PathBuf};

use crate::HarnessError;

/// 单个上下文文件的注入上限（按字符切，避免断开多字节中文）。
const MAX_CONTEXT_CHARS: usize = 8000;

/// 项目说明文件名（位于工作目录根）。
const PROJECT_FILE: &str = "AGENT.md";

/// 全局记忆文件路径；无法确定配置目录时返回 `None`。
pub fn memory_file() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("resolve-tui").join("MEMORY.md"))
}

/// 读取并按上限截断；文件不存在或内容为空白 → `None`。
fn read_capped(path: &Path) -> Option<String> {
    let s = std::fs::read_to_string(path).ok()?;
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if s.chars().count() <= MAX_CONTEXT_CHARS {
        return Some(s.to_string());
    }
    let head: String = s.chars().take(MAX_CONTEXT_CHARS).collect();
    Some(format!("{head}\n…（内容过长已截断）"))
}

/// 项目上下文（cwd 下 `AGENT.md`）；不存在 → `None`。
pub fn project_context() -> Option<String> {
    read_capped(Path::new(PROJECT_FILE))
}

/// 全局长期记忆内容；不存在 → `None`。
pub fn memory_context() -> Option<String> {
    read_capped(&memory_file()?)
}

/// 追加一条记忆（内容压成单行，自动建目录）。返回写入的文件路径。
/// 仅 TUI 的 `/remember` 命令调用；纯库构建下允许未使用。
#[cfg_attr(not(feature = "tui"), allow(dead_code))]
pub fn remember(text: &str) -> Result<PathBuf, HarnessError> {
    let path = memory_file().ok_or_else(|| HarnessError::other("无法确定配置目录"))?;
    remember_at(&path, text)?;
    Ok(path)
}

/// [`remember`] 的可测核心：写到指定路径。
#[cfg_attr(not(feature = "tui"), allow(dead_code))]
pub(crate) fn remember_at(path: &Path, text: &str) -> Result<(), HarnessError> {
    let one_line: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.is_empty() {
        return Err(HarnessError::other("记忆内容不能为空"));
    }
    let mut body = std::fs::read_to_string(path).unwrap_or_default();
    if !body.is_empty() && !body.ends_with('\n') {
        body.push('\n');
    }
    body.push_str(&format!("- {one_line}\n"));
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| HarnessError::other(format!("创建目录失败: {e}")))?;
    }
    // 长期记忆含个人事实，0600 落盘，避免同机其它用户读取。
    crate::agent::write_private(&path.to_string_lossy(), &body)
        .map_err(|e| HarnessError::other(format!("写入记忆失败: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remember_appends_and_reads_back() {
        let p = std::env::temp_dir().join(format!("harness_mem_{}.md", std::process::id()));
        let _ = std::fs::remove_file(&p);
        remember_at(&p, "喜欢简洁的中文回答").unwrap();
        remember_at(&p, "  多行   文本   会被压成   单行  ").unwrap();
        let body = std::fs::read_to_string(&p).unwrap();
        assert_eq!(body.lines().count(), 2);
        assert!(body.contains("- 喜欢简洁的中文回答"));
        assert!(body.contains("- 多行 文本 会被压成 单行"));

        // 读回：read_capped 应返回去尾空白后的完整内容。
        let got = read_capped(&p).unwrap();
        assert!(got.starts_with("- 喜欢简洁的中文回答"));

        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn empty_input_rejected_and_missing_file_is_none() {
        let p = std::env::temp_dir().join(format!("harness_mem_nil_{}", std::process::id()));
        assert!(remember_at(&p, "   ").is_err());
        assert_eq!(read_capped(&p), None);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn oversized_content_is_truncated() {
        let p = std::env::temp_dir().join(format!("harness_mem_big_{}", std::process::id()));
        let big = "字".repeat(MAX_CONTEXT_CHARS + 100);
        std::fs::write(&p, &big).unwrap();
        let got = read_capped(&p).unwrap();
        assert!(got.chars().count() < MAX_CONTEXT_CHARS + 200);
        assert!(got.contains("已截断"));
        let _ = std::fs::remove_file(&p);
    }
}
