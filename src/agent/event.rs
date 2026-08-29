//! agent 运行过程中对外发出的事件与审批应答类型。
//!
//! CLI 与 TUI 共用同一套 `AgentEvent`；主循环把进度、工具调用、用量等
//! 封装成事件广播出去，由调用方决定如何渲染。

use crate::model::InputItem;

/// 用户对一个工具审批请求的应答：`(call_id, approved)`。
pub type Approval = (String, bool);

/// agent 运行过程中对外发出的事件，CLI 与 TUI 共用同一套。
#[derive(Clone, Debug)]
pub enum AgentEvent {
    /// 模型流式产出的文本增量。
    Token(String),
    /// 模型请求一次工具调用。
    ToolCall { name: String, id: String },
    /// 工具执行结果。
    /// 工具执行结果。失败时 `preview` 携带错误摘要（单行、截断），
    /// 让 UI 能直接显示原因而不是只报 err。
    ToolResult {
        id: String,
        ok: bool,
        chars: usize,
        preview: Option<String>,
        /// 工具输出正文（已按 MAX_TOOL_CHARS 截断）。UI 在成功时可选展示，
        /// 便于用户直接看到长任务工具（如 pse-review）的完整产物。
        content: String,
    },
    /// 推理摘要（reasoning 模型才有）。
    Reasoning(String),
    /// 本轮回应的用量与是否触发工具，以及累计预算消耗。
    Usage {
        input_tokens: u64,
        output_tokens: u64,
        had_tools: bool,
        total_tokens: u64,
        max_tokens: u64,
    },
    /// 等待用户审批一次工具调用（仅交互式 TUI 会挂起等待应答）。
    ToolApproval {
        id: String,
        name: String,
        args: String,
    },
    /// 系统提示（如会话保存/加载结果）。
    System(String),
    /// 出错（循环已终止）。
    Error(String),
    /// 当前轮迭代序号。
    Iteration(usize),
    /// 切换推理摘要的展开/折叠展示（由 `/reasoning` 或 Ctrl-R 触发）。
    ToggleReasoning,
    /// 清空 TUI 可见历史与推理缓存（由 `/clear` 触发，仅影响展示层）。
    ClearScreen,
    /// 导出当前会话为 Markdown（由 `/export [路径]` 触发，TUI 专属）。
    Export(String),
    /// 一轮对话结束（给出最终答案或达上限）。
    Finished,
    /// 载入（或自动续接）会话后，把已恢复的历史回放到 TUI 可见记录，
    /// 否则续接后屏幕空白、用户误以为会话丢失（仅展示用，不含看门狗/状态含义）。
    Resumed(Vec<InputItem>),
    /// 启动时的能力面摘要：技能数 / 工具总数（内置+远端）/ 在线 MCP server 数。
    /// TUI 常驻显示在输入框标题栏；CLI 忽略。
    Capabilities {
        skills: usize,
        tools: usize,
        mcp_online: usize,
    },
    /// 输出一份 Markdown 文档（如 `/examples`）：TUI 按 Markdown 样式渲染，CLI 忽略。
    Document(String),
    /// 请求退出 TUI（由 `/quit` `/exit` `/q` 或裸写 `q`/`exit`/`quit` 触发）。
    /// TUI 事件循环据此置位 `should_quit`；CLI 忽略（CLI 走独立退出路径）。
    Quit,
}
