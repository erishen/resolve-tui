//! `Conversation::submit`：单次任务提交入口（快路径 → 主循环 → 事后学习）。
//!
//! 从 `agent/mod.rs` 抽出，使「对话状态」模块与其「提交逻辑」分文件维护。

use std::path::PathBuf;
#[cfg(feature = "codegen")]
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::{
    Config, HarnessError,
    model::InputItem,
    tools::builtin_tools,
};
use crate::agent::{AgentEvent, Approval, Conversation};

impl Conversation {
    /// 提交一条用户任务，驱动 agent 循环直到最终答案或迭代上限。
    ///
    /// `approval_rx` 用于工具审批模式：当 `Config::approve_tools` 为真时，
    /// 每次工具执行前会阻塞等待一条 `Approval` 应答。
    ///
    /// 有状态 / 无状态两条路径共用同一主循环 [`drive`](crate::agent::drive)，仅两处不同：
    /// - 发送的历史：有状态只发本轮新增项 + `previous_response_id`；
    ///   无状态发全量本地历史（兼容 Agnes 等不持久化的网关）。
    /// - 回灌位置：有状态只补 `function_call_output`；无状态需补
    ///   `function_call`（arguments 为 JSON 字符串）+ output 两项。
    pub async fn submit(
        &mut self,
        task: &str,
        config: &Config,
        tx: &mpsc::UnboundedSender<AgentEvent>,
        approval_rx: &mut mpsc::UnboundedReceiver<Approval>,
    ) -> Result<String, HarnessError> {
        // 确定性快路径：算术/单位/日期/进制/时间/统计等纯代码可解的问题，
        // 直接短路返回，省去一次 LLM 往返（设计对齐 resolve-harness 的 fastpath）。
        if let Some(fast) = crate::fastpath::try_fast_answer(task, None) {
            let _ = tx.send(AgentEvent::System(format!(
                "[快路径 {}] {}",
                fast.method, fast.detail
            )));
            let _ = tx.send(AgentEvent::Token(fast.answer.clone()));
            return Ok(fast.answer);
        }

        // codegen 快路径（零模型）：只查已持久化插件的缓存命中。
        // 检测器「生成」延后到主循环给出答案之后（answer-first）——
        // 开放性对话不再为一次注定 NONE 的生成白付整轮 LLM 延迟。
        // 插件目录来自 config（可按项目隔离）；None 时用系统默认位置。
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
            // 分层路由：检测器生成是结构化小任务，优先走配置的便宜快速模型；
            // 未配置（codegen_model=None）时沿用当前主模型。
            let gen_model = match &config.codegen_model {
                Some(m) => m.clone(),
                None => self.model.lock().map(|g| g.clone()).unwrap_or_default(),
            };
            Some((task.to_string(), gen_model, plugin_dir))
        } else {
            None
        };
        #[cfg(not(feature = "codegen"))]
        let _learn: Option<(String, String, Option<PathBuf>)> = None;

        // 内置工具在前，MCP 远端工具随后（暴露名不冲突）；剔除被 UI 禁用的。
        let mut tools = builtin_tools();
        tools.extend(self.extra_tools.iter().cloned());
        tools.retain(|t| !self.disabled_tools.contains(&t.name));
        self.stateful = config.stateful;
        // 本轮 system prompt 附录：项目上下文 + 长期记忆 + 技能索引/命中技能全文。
        let mut extra_instructions = super::build_extra_instructions(&self.skills, task);
        // 新一轮开始时清掉上一次的取消信号。
        self.cancel
            .store(false, std::sync::atomic::Ordering::SeqCst);
        // 每任务独立沙箱工作区：`<sandbox_dir>/task-<nanos>-<pid>/`，工具相对路径、
        // 写文件与 shell 命令全部落在该工作区，任务间互不覆盖（沙箱启用时生效）。
        super::init_task_workspace(self, config, tx);
        // 明确告知模型当前可写工作区，避免瞎猜路径。
        if let Some(policy) = self.task_policy.as_ref()
            && let Some(hint) = crate::sandbox::prompt(policy)
        {
            extra_instructions = Some(match extra_instructions {
                Some(base) => format!("{base}\n\n{hint}"),
                None => hint,
            });
        }

        let answer = if self.stateful {
            // 有状态模式不维护本地全量历史，只攒本轮新增项。
            let mut turn_items = vec![InputItem::message("user", task)];
            self.drive(
                config,
                &tools,
                &mut turn_items,
                tx,
                approval_rx,
                extra_instructions,
            )
            .await?
        } else {
            self.input.push(InputItem::message("user", task));
            let mut turn_items = Vec::new();
            self.drive(
                config,
                &tools,
                &mut turn_items,
                tx,
                approval_rx,
                extra_instructions,
            )
            .await?
        };

        // 答案已流式送达用户；此时才做「事后学习」，为下次同类问题沉淀零模型缓存。
        // 仅在成功回合学习：取消 / 出错 / 超迭代的轮次不追加 token 消耗。
        #[cfg(feature = "codegen")]
        if let Some((query, model, plugin_dir)) = learn {
            let cancel = Arc::clone(&self.cancel);
            if self.codegen_background {
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
}
