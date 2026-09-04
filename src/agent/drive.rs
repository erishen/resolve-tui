//! agent 主循环：请求 → 抽取工具调用 → 执行 → 回灌，直到最终答案或迭代上限。
//!
//! 与 `submit`（提交入口，见 `crate::agent`）共用同一个 `Conversation`，
//! 仅把循环本体与单步工具执行拆到本模块，便于阅读与单测。

use std::sync::atomic::Ordering;

use tokio::sync::mpsc;

use crate::agent::helpers::*;
use crate::agent::{AgentEvent, Approval, Conversation};
use crate::llm::{StreamOpts, create_response};
use crate::model::{Completion, FunctionCall, InputItem, ResponseTool};
use crate::sandbox::SandboxPolicy;
use crate::tools;
use crate::{Config, HarnessError};

impl Conversation {
    /// agent 主循环：请求 → 抽取工具调用 → 执行 → 回灌，直到最终答案或迭代上限。
    ///
    /// `pub(crate)` 因为提交入口 `submit`（`crate::agent`）会从另一个模块调用它。
    pub(crate) async fn drive(
        &mut self,
        config: &Config,
        tools: &[ResponseTool],
        turn_items: &mut Vec<InputItem>,
        tx: &mpsc::UnboundedSender<AgentEvent>,
        approval_rx: &mut mpsc::UnboundedReceiver<Approval>,
        extra_instructions: Option<String>,
    ) -> Result<String, HarnessError> {
        for iteration in 0..config.max_iterations {
            // 用户中途取消（Esc）：立即中止本轮，不消耗更多请求。
            if self.cancel.load(Ordering::SeqCst) {
                return Err(HarnessError::cancelled());
            }
            if let Some(budget) = self.over_budget(config) {
                return Err(HarnessError::llm(budget));
            }
            let _ = tx.send(AgentEvent::Iteration(iteration));

            let full_history: &[InputItem] = if self.stateful {
                turn_items.as_slice()
            } else {
                &self.input
            };
            // 无状态模式全量重发会随轮数线性膨胀 token；超限时按窗口裁剪
            // （有状态模式历史在服务端，不受此配置影响）。
            let history = windowed_history(full_history, config.history_max_items);
            let prev_id = if self.stateful {
                self.previous_id.as_deref()
            } else {
                None
            };

            let model = self.model.lock().unwrap_or_else(|e| e.into_inner()).clone();
            let completion = create_response(
                config,
                &model,
                history,
                tools,
                |delta| {
                    let _ = tx.send(AgentEvent::Token(delta.to_string()));
                },
                StreamOpts {
                    previous_response_id: prev_id,
                    tool_choice: tool_choice(config),
                    extra_instructions: extra_instructions.clone(),
                },
                &self.cancel,
            )
            .await?;

            // 仅在有状态模式下续接游标（无状态网关会 404）。
            if self.stateful {
                self.previous_id = completion.id.clone();
            }
            self.accumulate(&completion);
            emit_completion(&completion, self.total_tokens, config.max_tokens, tx);

            if completion.function_calls.is_empty() {
                if let Some(text) = completion.text {
                    return Ok(text);
                }
                return Err(HarnessError::llm(format!(
                    "model returned no content at iteration {iteration}"
                )));
            }

            for call in &completion.function_calls {
                self.run_tool_call(call, config, turn_items, tx, approval_rx)
                    .await;
            }
        }
        Err(HarnessError::max_iterations(config.max_iterations))
    }

    /// 审批模式入口：返回 `Some(output)` 表示被拒绝（已生成拒绝结果文本），
    /// 返回 `None` 表示已批准，调用方应继续执行工具。
    async fn maybe_approve(
        &mut self,
        call: &FunctionCall,
        config: &Config,
        tx: &mpsc::UnboundedSender<AgentEvent>,
        approval_rx: &mut mpsc::UnboundedReceiver<Approval>,
    ) -> Option<String> {
        if !config.approve_tools {
            return None;
        }
        let _ = tx.send(AgentEvent::ToolApproval {
            id: call.call_id.clone(),
            name: call.name.clone(),
            args: call.arguments.clone(),
        });
        // 阻塞直到 UI 给出应答；UI 断开（None）视为拒绝。
        let approved = approval_rx.recv().await.map(|(_, ok)| ok).unwrap_or(false);
        if approved {
            None
        } else {
            Some("用户拒绝了该工具调用".to_string())
        }
    }

    /// 执行单个工具调用：发起审批（若启用）、路由到 MCP 或内置沙箱、截断结果、
    /// 推送 `ToolResult` 事件，并把结果回灌到本轮历史（有状态）或全量历史（无状态）。
    async fn run_tool_call(
        &mut self,
        call: &FunctionCall,
        config: &Config,
        turn_items: &mut Vec<InputItem>,
        tx: &mpsc::UnboundedSender<AgentEvent>,
        approval_rx: &mut mpsc::UnboundedReceiver<Approval>,
    ) {
        let _ = tx.send(AgentEvent::ToolCall {
            name: call.name.clone(),
            id: call.call_id.clone(),
        });

        // 审批模式：阻塞等待用户确认；被拒则把拒绝结果回灌给模型。
        let denied = self.maybe_approve(call, config, tx, approval_rx).await;
        let output = match denied {
            Some(denial) => denial,
            None => {
                // MCP 工具按暴露名路由到远端 server；其余走内置沙箱工具。
                let is_remote = match &self.mcp {
                    Some(m) => m.read().await.routes(&call.name),
                    None => false,
                };
                let result = if is_remote {
                    match &self.mcp {
                        Some(m) => m.read().await.call(&call.name, &call.arguments).await,
                        None => unreachable!("is_remote 为真时 manager 必然存在"),
                    }
                } else {
                    execute_tool(call, self.effective_policy(config)).await
                };
                let (ok, output, preview) = match result {
                    Ok(o) => {
                        let truncated = tools::truncate_output(o);
                        (true, truncated, None)
                    }
                    Err(e) => {
                        let msg = e.to_string();
                        let truncated = tools::truncate_output(format!("error: {msg}"));
                        (false, truncated, Some(flatten_error(&msg)))
                    }
                };
                let chars = output.len();
                let content = output.clone();
                // 长任务报告工具（pse-review）的成功结果落盘到任务工作区，方便用户
                // 留存/打开；其余工具不落盘，避免污染工作目录。
                if ok
                    && call.name.contains("pse-review")
                    && let Some(cwd) = self.effective_policy(config).cwd.as_deref()
                {
                    let path = cwd.join("weekly_review.md");
                    if std::fs::write(&path, &content).is_ok() {
                        let _ = tx.send(AgentEvent::System(format!(
                            "[pse-review] 周报已保存 → {}",
                            path.display()
                        )));
                    }
                }
                let _ = tx.send(AgentEvent::ToolResult {
                    id: call.call_id.clone(),
                    ok,
                    chars,
                    preview,
                    content,
                });
                output
            }
        };

        if self.stateful {
            turn_items.push(InputItem::function_call_output(
                call.call_id.clone(),
                output,
            ));
        } else {
            // 回灌：先 function_call（arguments 保持 JSON 字符串，id 必须带上），再 output。
            self.input.push(InputItem::function_call(
                call.call_id.clone(),
                call.name.clone(),
                call.arguments.clone(),
                call.id.clone(),
            ));
            self.input.push(InputItem::function_call_output(
                call.call_id.clone(),
                output,
            ));
        }
    }

    /// 若超过 token 预算，返回错误信息，否则 `None`。
    fn over_budget(&self, config: &Config) -> Option<String> {
        if config.max_tokens > 0 && self.total_tokens >= config.max_tokens {
            Some(format!(
                "预算超限：已用 {} >= 上限 {}",
                self.total_tokens, config.max_tokens
            ))
        } else {
            None
        }
    }

    /// 生效的沙箱策略：当前任务独立工作区策略优先，否则用配置策略。
    pub(crate) fn effective_policy<'a>(&'a self, config: &'a Config) -> &'a SandboxPolicy {
        self.task_policy.as_ref().unwrap_or(&config.policy)
    }

    /// 累加本轮用量。
    ///
    /// 无状态模式下需把助手本轮内容回灌到本地全量历史，否则下一轮发回去的
    /// `input` 会缺少 assistant 消息（只剩 user + function_call），上游会 400。
    /// 工具调用轮由 drive 负责回灌 function_call + output，这里只补纯文本回复。
    pub(crate) fn accumulate(&mut self, completion: &Completion) {
        self.total_tokens += completion.usage.input_tokens + completion.usage.output_tokens;
        if !self.stateful
            && completion.function_calls.is_empty()
            && let Some(text) = &completion.text
        {
            self.input.push(InputItem::message("assistant", text));
        }
    }
}
