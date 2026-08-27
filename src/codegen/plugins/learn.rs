//! codegen 主流程：缓存查找（零模型）→ 事后学习（模型生成 + 隔离执行 + 持久化）。
//!
//! 答案先行架构下，[`codegen_cached_answer`] 在主循环每轮提交时同步调用，
//! [`codegen_learn`] 在答案返回后再后台调用，二者通过 [`super::cache`] 与
//! [`super::registry`] 的纯函数协作。

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use crate::codegen::engine;
use crate::codegen::extract;
use crate::codegen::sandbox;
use crate::llm::complete_once;
use crate::{Config, HarnessError};

use super::CODE_GEN_SYSTEM;
use super::cache::{enforce_plugin_cap, load_plugins, prefilter_plugins, record_hits};
use super::default_plugin_dir;
use super::registry::save_plugin;

/// 缓存插件查找（**零模型**）：在隔离子进程里批量执行已持久化的检测器，
/// 命中即返回答案。目录为空或全部未命中时返回 `None`，不产生任何模型请求。
pub async fn codegen_cached_answer(query: &str, plugin_dir: Option<&Path>) -> Option<String> {
    let dir = plugin_dir
        .map(Path::to_path_buf)
        .unwrap_or_else(default_plugin_dir);
    // 目录扫描/编译/粗筛在阻塞线程池执行：这是每轮提交都会走的热路径，
    // 不应占住 tokio worker（插件多时 read_dir+编译可感知）。
    let q = query.to_string();
    let scan_dir = dir.clone();
    let candidates: Vec<(String, String)> = tokio::task::spawn_blocking(move || {
        let plugins = load_plugins(&scan_dir);
        prefilter_plugins(&q, &plugins)
            .into_iter()
            .map(|p| (p.name.clone(), p.src.clone()))
            .collect()
    })
    .await
    .ok()?;
    if candidates.is_empty() {
        return None;
    }
    let sources: Vec<String> = candidates.iter().map(|(_, s)| s.clone()).collect();
    let hit = sandbox::run_in_subprocess(query, &sources, Duration::from_secs(3)).await?;
    // 命中统计：写盘放阻塞线程池；失败静默。旧协议无下标时跳过计数，不影响答案返回。
    if let Some(i) = hit.index
        && let Some((name, _)) = candidates.get(i)
    {
        let stats_dir = dir;
        let name = name.clone();
        let _ = tokio::task::spawn_blocking(move || record_hits(&stats_dir, &[name])).await;
    }
    Some(hit.answer)
}

/// 「事后学习」路径：让模型为该问题生成检测器，编译校验 + 隔离执行通过后持久化。
///
/// answer-first 架构下在主循环给出答案**之后**调用——开放性对话不再为一次
/// 注定 `NONE` 的生成白付整轮 LLM 延迟。CLI 单次进程应在退出前 await 本函数
/// 以保证插件落盘；TUI 等常驻进程可 `tokio::spawn` 后台执行。
pub async fn codegen_learn(
    config: &Config,
    model: &str,
    query: &str,
    plugin_dir: Option<&Path>,
    cancel: &AtomicBool,
) -> Result<Option<String>, HarnessError> {
    let dir = plugin_dir
        .map(Path::to_path_buf)
        .unwrap_or_else(default_plugin_dir);
    codegen_solve(config, model, query, &dir, 2, cancel).await
}

/// 组合入口：先查缓存（零模型），未命中再生成并持久化（原同步行为）。
/// 供端到端测试与脚本使用；agent 主流程改用 `codegen_cached_answer` +
/// `codegen_learn` 两段式以实现答案先行。
pub async fn try_codegen(
    query: &str,
    config: &Config,
    model: &str,
    plugin_dir: Option<&Path>,
) -> Result<Option<String>, HarnessError> {
    let cancel = AtomicBool::new(false);
    if let Some(ans) = codegen_cached_answer(query, plugin_dir).await {
        return Ok(Some(ans));
    }
    codegen_learn(config, model, query, plugin_dir, &cancel).await
}

/// 让模型生成检测器并尝试验证/执行；最多 `max_attempts` 次（首次失败后带反馈重试）。
async fn codegen_solve(
    config: &Config,
    model: &str,
    query: &str,
    dir: &Path,
    max_attempts: usize,
    cancel: &AtomicBool,
) -> Result<Option<String>, HarnessError> {
    let mut last_error = "detect() 未命中该问题".to_string();
    for attempt in 0..max_attempts.max(1) {
        // 用户已取消：立即停止学习，不再消耗请求。
        if cancel.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(HarnessError::cancelled());
        }
        let mut prompt = query.to_string();
        if attempt > 0 {
            prompt = format!(
                "{prompt}\n\n你上一次生成的 detect() 无法匹配该问题（{last_error}）。请重新生成能返回答案的函数。"
            );
        }
        let resp = match complete_once(config, model, CODE_GEN_SYSTEM, &prompt, cancel).await {
            Ok(r) => r,
            // 用户主动取消：向上传播（调用方据此停止后续学习）。
            Err(HarnessError::Cancelled) => return Err(HarnessError::cancelled()),
            // 网络/鉴权失败：非致命，回退普通 agent 循环。
            Err(_) => return Ok(None),
        };
        let source = match extract::extract_code(&resp) {
            Some(s) => s,
            // 模型判定不可确定性求解（NONE）→ 停止。
            None => return Ok(None),
        };
        // 仅做编译期校验（安全、不执行，可拿到真实错误回灌给模型）。
        if let Err(err) = compile_only(&source) {
            let err = err.to_string().chars().take(400).collect::<String>();
            last_error = format!("上一版代码编译失败：{err}");
            continue;
        }
        // 隔离执行候选检测器（子进程，超时即杀，绝不泄漏/占资源）。
        let sources = vec![source.clone()];
        if let Some(hit) = sandbox::run_in_subprocess(query, &sources, Duration::from_secs(3)).await
        {
            // 落盘 + 首次命中计数 + 上限淘汰，一起放阻塞线程池。
            let save_dir = dir.to_path_buf();
            let src = source.clone();
            let q = query.to_string();
            let _ = tokio::task::spawn_blocking(move || {
                if let Ok(name) = save_plugin(&src, &save_dir, &q) {
                    record_hits(&save_dir, &[name]);
                    enforce_plugin_cap(&save_dir);
                }
            })
            .await;
            return Ok(Some(hit.answer));
        }
        last_error = "上一版代码能编译，但未命中该问题".to_string();
    }
    Ok(None)
}

/// 仅编译源码以拿到错误（不执行），供生成重试时反馈；编译阶段不会陷入死循环，安全。
fn compile_only(source: &str) -> Result<(), HarnessError> {
    engine::build_engine()
        .compile(source)
        .map(|_| ())
        .map_err(|e| HarnessError::other(e.to_string()))
}
