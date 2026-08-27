//! agent 的纯函数辅助：system 附录组装、历史窗口、错误压平、工具执行等。
//!
//! 这些函数无状态（不碰 `Conversation`），便于单测与复用；主循环 `drive`
//! （见 `crate::agent::drive`）与提交入口 `submit`（见 `crate::agent`）都依赖它们。

use tokio::sync::mpsc;

use crate::Config;
use crate::HarnessError;
use crate::agent::AgentEvent;
use crate::model::{Completion, FunctionCall, InputItem};
use crate::sandbox::SandboxPolicy;
use crate::skills::{self, Skill};
use crate::tools::execute;

/// 组装本轮 system prompt 附录：项目上下文（AGENT.md）+ 长期记忆（MEMORY.md）
/// + 技能注入。三者皆空时返回 `None`，不发送多余内容。
///
/// 每轮重读文件：上下文文件很小（有截断上限），相对 LLM 延迟可忽略；
/// 换来的好处是用户改完 AGENT.md / `/remember` 后下一轮立即生效，无需重启。
pub(crate) fn build_extra_instructions(skills: &[Skill], task: &str) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    if let Some(p) = crate::memory::project_context() {
        parts.push(format!(
            "以下是当前项目的说明文件（AGENT.md），请遵循其中的约定：\n{p}"
        ));
    }
    if let Some(m) = crate::memory::memory_context() {
        parts.push(format!(
            "以下是用户的长期记忆（MEMORY.md），回答时请参考：\n{m}"
        ));
    }
    if let Some(sk) = skills::prompt_appendix(skills, task) {
        parts.push(sk);
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n\n"))
    }
}

/// 无状态模式下发给模型的历史窗口：超出 `max_items` 时从尾部保留，
/// 并把起点向后滑动到最近的 user 消息边界——绝不能从 `function_call`
/// 与其 `function_call_output` 的中间开始（配对断裂会被上游 400）。
/// 单轮自身超长时窗口会略微超出上限（正确性优先于严格条数）。
/// `max_items == 0` 表示不限制，原样返回。
pub(crate) fn windowed_history(items: &[InputItem], max_items: usize) -> &[InputItem] {
    if max_items == 0 || items.len() <= max_items {
        return items;
    }
    let mut start = items.len() - max_items;
    while start < items.len() && !is_safe_boundary(&items[start]) {
        start += 1;
    }
    &items[start..]
}

/// 是否为可安全开始的窗口起点：仅 user 消息。
fn is_safe_boundary(item: &InputItem) -> bool {
    matches!(item, InputItem::Message { role, .. } if role == "user")
}

/// 把错误文本压成单行并截断（换行 → 空格；上限 160 字符，按字符切避免断开中文）。
pub(crate) fn flatten_error(e: &str) -> String {
    let one_line = e.replace(['\n', '\r'], " ");
    let collapsed = one_line.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= 160 {
        collapsed
    } else {
        let head: String = collapsed.chars().take(160).collect();
        format!("{head}…")
    }
}

/// 调试开关：强制模型至少调用一个工具（走 `tool_choice: "required"`）。
pub(crate) fn tool_choice(config: &Config) -> Option<&'static str> {
    if config.force_tools {
        Some("required")
    } else {
        None
    }
}

/// 把一轮 Completion 的推理摘要与用量广播出去（供 CLI/TUI 展示）。
pub(crate) fn emit_completion(
    completion: &Completion,
    total_tokens: u64,
    max_tokens: u64,
    tx: &mpsc::UnboundedSender<AgentEvent>,
) {
    if let Some(reasoning) = &completion.reasoning {
        let _ = tx.send(AgentEvent::Reasoning(reasoning.clone()));
    }
    let _ = tx.send(AgentEvent::Usage {
        input_tokens: completion.usage.input_tokens,
        output_tokens: completion.usage.output_tokens,
        had_tools: !completion.function_calls.is_empty(),
        total_tokens,
        max_tokens,
    });
}

/// 把模型请求的工具调用路由到本地（沙箱）执行。
pub(crate) async fn execute_tool(
    call: &FunctionCall,
    policy: &SandboxPolicy,
) -> Result<String, HarnessError> {
    execute(&call.name, &call.arguments, policy).await
}
