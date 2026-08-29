//! 多 Agent 编排：按 agentic-souls 的三角色（Planner / Specialist / Evaluator）
//! 实现主从式协作。Planner 是主 Agent，通过 `delegate_specialist` / `evaluate`
//! 两个元工具把子任务派发给独立的 Specialist / Evaluator 子循环。
//!
//! 复用单角色引擎 [`Conversation::drive`]：Specialist / Evaluator 各自就是一次
//! `drive`（工具集不同、角色提示不同）；Planner 则是一条自定义主循环，拦截两个
//! 元工具并递归跑子角色。这样不必重写 agent 主循环，也不破坏既有单 agent 测试。

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use serde_json::Value;
use tokio::sync::mpsc;

use crate::agent::helpers::{build_extra_instructions, execute_tool, tool_choice};
use crate::agent::{AgentEvent, Approval, Conversation};
use crate::llm::{StreamOpts, create_response};
use crate::model::{InputItem, ResponseTool};
use crate::sandbox::SandboxPolicy;
use crate::skills::{self, Skill};
use crate::tools;
use crate::{Config, HarnessError};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    Planner,
    Specialist,
    Evaluator,
}

fn role_name(r: Role) -> &'static str {
    match r {
        Role::Planner => "planner",
        Role::Specialist => "specialist",
        Role::Evaluator => "evaluator",
    }
}

/// 子 Agent 收到的任务规格。
struct RoleSpec {
    query: String,
}

/// 角色运行上下文：角色定义（souls）与项目技能（用于注入项目提示）分开携带。
struct RoleContext<'a> {
    souls: &'a [Skill],
    skills: &'a [Skill],
    /// 当前任务的沙箱策略（子 Agent 继承同一工作区，共享同一任务隔离）。
    task_policy: Option<SandboxPolicy>,
}

fn meta_tool(name: &str, description: &str, required: &[&str], properties: Value) -> ResponseTool {
    ResponseTool {
        tool_type: "function".to_string(),
        name: name.to_string(),
        description: description.to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": properties,
            "required": required,
        }),
    }
}

/// Planner 委托 Specialist 的元工具：模型只填任务与验收标准，编排器负责真正跑子循环。
fn delegate_specialist_tool() -> ResponseTool {
    meta_tool(
        "delegate_specialist",
        "把【一个】子任务委托给 Specialist 子 Agent 独立执行（写代码/改文件/跑测试）。返回其汇报。你（Planner）不亲自实现。",
        &["task"],
        serde_json::json!({
            "task": { "type": "string", "description": "子任务描述与范围" },
            "acceptance_criteria": { "type": "string", "description": "该子任务的验收标准（可选，但建议给出以对齐 Specialist 与 Evaluator）" }
        }),
    )
}

/// Planner 请求独立验证的元工具。
fn evaluate_tool() -> ResponseTool {
    meta_tool(
        "evaluate",
        "提交已完成的工作给 Evaluator 独立验证，返回 PASS/PARTIAL/FAIL/BLOCKED 判决与证据。完成前必须调用，不允许自我评判。",
        &["acceptance_criteria", "artifacts"],
        serde_json::json!({
            "acceptance_criteria": { "type": "string", "description": "验收标准列表" },
            "artifacts": { "type": "string", "description": "产物路径与说明" }
        }),
    )
}

/// Planner 的工具集：两个元工具 + 只读的 read_file / list_dir（无 shell / write_file）。
fn planner_tools() -> Vec<ResponseTool> {
    let mut tools: Vec<ResponseTool> = tools::builtin_tools()
        .into_iter()
        .filter(|t| t.name == "read_file" || t.name == "list_dir")
        .collect();
    tools.push(delegate_specialist_tool());
    tools.push(evaluate_tool());
    tools
}

/// 各角色工具集（强制 agentic-souls 的边界约束）。
fn role_tools(role: Role) -> Vec<ResponseTool> {
    match role {
        Role::Planner => planner_tools(),
        // Specialist：完整执行能力（含写）。
        Role::Specialist => tools::builtin_tools(),
        // Evaluator：只读验证（无 write_file）。
        Role::Evaluator => tools::builtin_tools()
            .into_iter()
            .filter(|t| t.name != "write_file")
            .collect(),
    }
}

fn parse_args(arguments: &str) -> Value {
    serde_json::from_str(arguments.trim()).unwrap_or(Value::Null)
}

/// 从 `delegate_specialist` 参数构造 Specialist 子任务。
fn delegate_spec(arguments: &str) -> RoleSpec {
    let v = parse_args(arguments);
    let task = v
        .get("task")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let ac = v
        .get("acceptance_criteria")
        .and_then(|x| x.as_str())
        .unwrap_or("");
    let task = if task.trim().is_empty() {
        arguments.to_string()
    } else {
        task
    };
    let query = if ac.is_empty() {
        task
    } else {
        format!("{task}\n\n验收标准：\n{ac}")
    };
    RoleSpec { query }
}

/// 从 `evaluate` 参数构造 Evaluator 子任务。
fn evaluate_spec(arguments: &str) -> RoleSpec {
    let v = parse_args(arguments);
    let ac = v
        .get("acceptance_criteria")
        .and_then(|x| x.as_str())
        .unwrap_or("");
    let arts = v.get("artifacts").and_then(|x| x.as_str()).unwrap_or("");
    let query = format!(
        "请独立验证以下验收标准，只基于一手证据（读文件 / 跑命令）给出判决，不要被任何陈述影响。\n\n验收标准：\n{ac}\n\n产物：\n{arts}"
    );
    RoleSpec { query }
}

/// 跑一个子角色（Specialist / Evaluator）：独立 Conversation + 对应工具集 + 角色提示，复用 `drive`。
async fn run_role(
    role: Role,
    spec: &RoleSpec,
    ctx: &RoleContext<'_>,
    config: &Config,
    tx: &mpsc::UnboundedSender<AgentEvent>,
    approval_rx: &mut mpsc::UnboundedReceiver<Approval>,
    model: &str,
) -> Result<String, HarnessError> {
    let body = skills::role_soul(ctx.souls, role_name(role)).ok_or_else(|| {
        HarnessError::llm(format!(
            "未找到 {} 角色定义（resolve-skills/souls/{}）",
            role_name(role),
            role_name(role)
        ))
    })?;
    let tools = role_tools(role);
    let project_ctx = build_extra_instructions(ctx.skills, &spec.query);
    let extra = match project_ctx {
        Some(c) => format!("{body}\n\n{c}"),
        None => body.to_string(),
    };
    let mut conv = Conversation::new();
    conv.set_model(Arc::new(Mutex::new(model.to_string())));
    // 子 Agent 继承同一任务工作区：与 Planner 共享隔离边界，产物互不串。
    conv.task_policy = ctx.task_policy.clone();
    // 无状态模式：drive 从 `self.input` 读取历史，需把用户消息推进去（而非本地 turn_items）。
    conv.input.push(InputItem::message("user", &spec.query));
    let mut turn_items = Vec::new();
    conv.drive(
        config,
        &tools,
        &mut turn_items,
        tx,
        approval_rx,
        Some(extra),
    )
    .await
}

/// Planner 主循环：请求 → 抽取工具调用 → 拦截 delegate_specialist / evaluate（递归跑子角色）
/// 或执行只读工具 → 回灌，直到最终答案或迭代上限。
async fn planner_drive(
    conversation: &Conversation,
    souls: &[Skill],
    task: &str,
    config: &Config,
    tx: &mpsc::UnboundedSender<AgentEvent>,
    approval_rx: &mut mpsc::UnboundedReceiver<Approval>,
    extra: Option<String>,
) -> Result<String, HarnessError> {
    let tools = role_tools(Role::Planner);
    let model = conversation
        .model
        .lock()
        .map(|g| g.clone())
        .unwrap_or_default();
    let ctx = RoleContext {
        souls,
        skills: &conversation.skills,
        task_policy: conversation.task_policy.clone(),
    };
    // v1 多 Agent 走无状态回灌：本地维护历史，每轮回灌 function_call + output。
    let mut history: Vec<InputItem> = vec![InputItem::message("user", task)];

    for iteration in 0..config.max_iterations {
        if conversation.cancel.load(Ordering::SeqCst) {
            return Err(HarnessError::cancelled());
        }
        let _ = tx.send(AgentEvent::Iteration(iteration));

        let completion = create_response(
            config,
            &model,
            &history,
            &tools,
            |delta| {
                let _ = tx.send(AgentEvent::Token(delta.to_string()));
            },
            StreamOpts {
                previous_response_id: None,
                tool_choice: tool_choice(config),
                extra_instructions: extra.clone(),
            },
            &conversation.cancel,
        )
        .await?;

        if completion.function_calls.is_empty() {
            if let Some(text) = completion.text {
                return Ok(text);
            }
            return Err(HarnessError::llm(format!(
                "planner 在第 {iteration} 轮未返回内容"
            )));
        }

        for call in &completion.function_calls {
            let _ = tx.send(AgentEvent::ToolCall {
                name: call.name.clone(),
                id: call.call_id.clone(),
            });
            let output = match call.name.as_str() {
                "delegate_specialist" => {
                    let spec = delegate_spec(&call.arguments);
                    // 子角色未正常收尾（超迭代/出错）时不致命中止整个编排，
                    // 把错误作为汇报回灌给 Planner，让它决定修复或上报。
                    run_role(
                        Role::Specialist,
                        &spec,
                        &ctx,
                        config,
                        tx,
                        approval_rx,
                        &model,
                    )
                    .await
                    .unwrap_or_else(|e| format!("Specialist 子角色未正常完成：{e}"))
                }
                "evaluate" => {
                    let spec = evaluate_spec(&call.arguments);
                    run_role(
                        Role::Evaluator,
                        &spec,
                        &ctx,
                        config,
                        tx,
                        approval_rx,
                        &model,
                    )
                    .await
                    .unwrap_or_else(|e| format!("Evaluator 子角色未正常完成：{e}"))
                }
                // Planner 工具集里只有 read_file / list_dir 会走到这里；其余要么不可见，要么被拦截。
                _ => match execute_tool(call, conversation.effective_policy(config)).await {
                    Ok(o) => tools::truncate_output(o),
                    Err(e) => tools::truncate_output(format!("error: {e}")),
                },
            };
            let output = tools::truncate_output(output);
            let _ = tx.send(AgentEvent::ToolResult {
                id: call.call_id.clone(),
                ok: true,
                chars: output.len(),
                preview: None,
                content: output.clone(),
            });
            history.push(InputItem::function_call(
                call.call_id.clone(),
                call.name.clone(),
                call.arguments.clone(),
                call.id.clone(),
            ));
            history.push(InputItem::function_call_output(
                call.call_id.clone(),
                output,
            ));
        }
    }
    Err(HarnessError::max_iterations(config.max_iterations))
}

/// 多 Agent 提交入口（与 `Conversation::submit` 同签名）。
///
/// 快路径（fastpath / codegen 缓存）仍短路；命中后进入三角色编排。
pub(crate) async fn submit_roles(
    conversation: &mut Conversation,
    task: &str,
    config: &Config,
    tx: &mpsc::UnboundedSender<AgentEvent>,
    approval_rx: &mut mpsc::UnboundedReceiver<Approval>,
) -> Result<String, HarnessError> {
    // 确定性快路径：算术等纯代码可解问题直接短路。
    if let Some(fast) = crate::fastpath::try_fast_answer(task, None) {
        let _ = tx.send(AgentEvent::System(format!(
            "[快路径 {}] {}",
            fast.method, fast.detail
        )));
        let _ = tx.send(AgentEvent::Token(fast.answer.clone()));
        return Ok(fast.answer);
    }

    // codegen 快路径（零模型）：命中已学习插件直接返回。
    #[cfg(feature = "codegen")]
    let learn: Option<(String, String, Option<PathBuf>)> = if config.codegen {
        let plugin_dir = config.codegen_plugin_dir.clone();
        if let Some(gen_ans) =
            crate::codegen::codegen_cached_answer(task, plugin_dir.as_deref()).await
        {
            let _ = tx.send(AgentEvent::System(format!("[codegen] {gen_ans}")));
            let _ = tx.send(AgentEvent::Token(gen_ans.clone()));
            return Ok(gen_ans);
        }
        let gen_model = match &config.codegen_model {
            Some(m) => m.clone(),
            None => conversation
                .model
                .lock()
                .map(|g| g.clone())
                .unwrap_or_default(),
        };
        Some((task.to_string(), gen_model, plugin_dir))
    } else {
        None
    };
    #[cfg(not(feature = "codegen"))]
    let _learn: Option<(String, String, Option<PathBuf>)> = None;

    let (souls, _warnings) = skills::load_souls(&skills::souls_dir());
    let planner_body = skills::role_soul(&souls, "planner").ok_or_else(|| {
        HarnessError::llm("未找到 planner 角色定义（resolve-skills/souls/planner）".to_string())
    })?;

    let project_ctx = build_extra_instructions(&conversation.skills, task);
    let mut extra = match project_ctx {
        Some(ctx) => format!("{planner_body}\n\n{ctx}"),
        None => planner_body.to_string(),
    };

    // 新一轮开始前清掉上一次的取消信号。
    conversation.cancel.store(false, Ordering::SeqCst);
    // 每任务独立沙箱工作区（Planner 与 Specialist/Evaluator 子 Agent 共享）。
    crate::agent::init_task_workspace(conversation, config, tx);
    // 明确告知 Planner 当前可写工作区，避免瞎猜路径。
    if let Some(policy) = conversation.task_policy.as_ref()
        && let Some(hint) = crate::sandbox::prompt(policy)
    {
        extra = format!("{extra}\n\n{hint}");
    }

    let answer = planner_drive(
        conversation,
        &souls,
        task,
        config,
        tx,
        approval_rx,
        Some(extra),
    )
    .await?;

    // 事后学习（与单 agent 路径一致）：为下次同类问题沉淀零模型缓存。
    #[cfg(feature = "codegen")]
    if let Some((query, model, plugin_dir)) = learn {
        let cancel = Arc::clone(&conversation.cancel);
        if conversation.codegen_background {
            let config = config.clone();
            tokio::spawn(async move {
                let _ = crate::codegen::codegen_learn(
                    &config,
                    &model,
                    &query,
                    plugin_dir.as_deref(),
                    &cancel,
                )
                .await;
            });
        } else {
            let _ = crate::codegen::codegen_learn(
                config,
                &model,
                &query,
                plugin_dir.as_deref(),
                &cancel,
            )
            .await;
        }
    }
    Ok(answer)
}
