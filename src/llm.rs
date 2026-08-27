use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures::StreamExt;
use serde_json::{Value, json};

use crate::{
    Config, HarnessError,
    model::{Completion, InputItem, Response, ResponseTool},
};

/// 全局复用的 HTTP 客户端：连接池避免每轮重建 TLS 连接。
/// 总超时 300s 兜底防挂死（覆盖绝大多数生成；超长流式会被切断并报错）。
static HTTP: OnceLock<reqwest::Client> = OnceLock::new();

fn http_client() -> reqwest::Client {
    HTTP.get_or_init(|| {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(300))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new())
    })
    .clone()
}

/// 基础系统提示词：工具清单从 `tools` 参数**动态生成**，而不是手写硬编码——
/// 这样给 tools.rs 新增工具时，系统提示会自动跟上，不会漏告知模型（防漂移）。
fn base_instructions(tools: &[ResponseTool]) -> String {
    let names = tools
        .iter()
        .map(|t| t.name.as_str())
        .collect::<Vec<_>>()
        .join(" / ");
    format!(
        "你是一个运行在用户本机的 agent harness。\n\
         你可以调用工具（{names}）来完成用户的任务：\n\
         - 需要执行命令或查看真实环境时调用对应工具；\n\
         - 工具结果会回传给你，请基于结果继续推理；\n\
         - 任务完成后用一段简洁的中文给出最终答案，不要再调用工具。"
    )
}

/// 流式请求的可选项：`previous_response_id` 与 `tool_choice`。
pub(crate) struct StreamOpts<'a> {
    pub(crate) previous_response_id: Option<&'a str>,
    pub(crate) tool_choice: Option<&'static str>,
    /// 追加到基础系统提示之后的内容（skills 索引 + 命中技能全文）。
    pub(crate) extra_instructions: Option<String>,
}

/// 调一次 `/responses`（SSE 流式），增量文本经 `on_token` 转发，
/// 完成时返回抽取好的 `Completion`。
pub async fn create_response<F>(
    config: &Config,
    model: &str,
    input: &[InputItem],
    tools: &[ResponseTool],
    mut on_token: F,
    opts: StreamOpts<'_>,
    cancel: &AtomicBool,
) -> Result<Completion, HarnessError>
where
    F: FnMut(&str),
{
    let instructions = match &opts.extra_instructions {
        Some(extra) => format!("{}\n\n{extra}", base_instructions(tools)),
        None => base_instructions(tools),
    };
    let mut body = json!({
        "model": model,
        "instructions": instructions,
        "input": input,
        "tools": tools,
        "stream": true,
        "store": false,
    });
    if let Some(prev) = opts.previous_response_id {
        body["previous_response_id"] = json!(prev);
    }
    if let Some(tc) = opts.tool_choice {
        body["tool_choice"] = json!(tc);
    }

    let url = format!("{}/responses", config.api_base.trim_end_matches('/'));
    let client = http_client();
    let resp = post_with_retry(&client, &url, &body, &config.api_key).await?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        let preview: String = text.chars().take(500).collect();
        return Err(HarnessError::llm(format!("status {status}: {preview}")));
    }

    // 逐行解析 SSE：`data: {...}`；completed 时返回，failed 时报错。
    let mut stream = resp.bytes_stream();
    // 以「字节」为单位缓冲，而不是逐网络分包解码：
    // 多字节 UTF-8 字符（如中文）可能被切在两个 chunk 边界上，
    // 若对每个 chunk 单独 from_utf8_lossy 会把残缺字节替换成 U+FFFD 乱码。
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = stream.next().await {
        // 用户中途取消：尽快中止流式，避免无谓的网络等待与 token 消耗。
        if cancel.load(Ordering::SeqCst) {
            return Err(HarnessError::cancelled());
        }
        let chunk = chunk.map_err(|e| HarnessError::llm(format!("流中断: {e}")))?;
        buf.extend_from_slice(&chunk);
        while let Some(line) = take_line(&mut buf) {
            if let Some(completion) = handle_event(&line, &mut on_token)? {
                return Ok(completion);
            }
        }
    }
    Err(HarnessError::llm("流结束但未收到 response.completed"))
}

/// 一次性纯文本补全（非流式）：用于 codegen 这类只需单个答案、不需要工具的场景。
/// 关闭 `stream`，直接解析完整 `Response` 取出文本；任何失败都上抛 `HarnessError`。
///
/// `cancel`：非流式请求无法从流中感知取消，改为在「发起前」与「等待响应/读体期间」
/// 以 100ms 轮询取消信号——一旦置位立即掐断连接并返回 `Cancelled`，
/// 避免 codegen 学习阶段按 Esc 无响应地干等。
#[cfg_attr(not(feature = "codegen"), allow(dead_code))]
pub async fn complete_once(
    config: &Config,
    model: &str,
    system: &str,
    prompt: &str,
    cancel: &AtomicBool,
) -> Result<String, HarnessError> {
    if cancel.load(Ordering::SeqCst) {
        return Err(HarnessError::cancelled());
    }
    let body = json!({
        "model": model,
        "instructions": system,
        "input": [InputItem::message("user", prompt)],
        "tools": [],
        "stream": false,
        "store": false,
    });
    let url = format!("{}/responses", config.api_base.trim_end_matches('/'));
    let client = http_client();
    let resp = tokio::select! {
        r = post_with_retry(&client, &url, &body, &config.api_key) => r?,
        _ = cancelled_poll(cancel) => return Err(HarnessError::cancelled()),
    };

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        let preview: String = text.chars().take(500).collect();
        return Err(HarnessError::llm(format!("status {status}: {preview}")));
    }

    let v: Value = tokio::select! {
        r = resp.json::<Value>() => {
            r.map_err(|e| HarnessError::llm(format!("解析 JSON 失败: {e}")))?
        }
        _ = cancelled_poll(cancel) => return Err(HarnessError::cancelled()),
    };
    let response: Response =
        serde_json::from_value(v).map_err(|e| HarnessError::llm(format!("解析响应失败: {e}")))?;
    if let Some(err) = &response.error {
        return Err(HarnessError::llm(format!("服务端错误: {}", err.message)));
    }
    Ok(Completion::from_response(&response)
        .text
        .unwrap_or_default())
}

/// 轮询取消信号直至置位（配合 `tokio::select!` 用作可中断的等待分支）。
#[cfg_attr(not(feature = "codegen"), allow(dead_code))]
async fn cancelled_poll(cancel: &AtomicBool) {
    loop {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if cancel.load(Ordering::SeqCst) {
            return;
        }
    }
}

/// 从字节缓冲取出下一条完整行（不含换行符）；不足一整行时返回 None。
/// 换行符不会出现在多字节 UTF-8 序列内部，因此按 \n 切分再解码是安全的。
fn take_line(buf: &mut Vec<u8>) -> Option<String> {
    let idx = buf.iter().position(|&b| b == b'\n')?;
    let line_bytes: Vec<u8> = buf.drain(..=idx).collect();
    Some(String::from_utf8_lossy(&line_bytes).trim_end().to_string())
}

/// 对 429 / 5xx 与连接类网络错误做指数退避重试（0.8s → 1.6s）；
/// 服务端给出 `Retry-After` 时优先遵循（封顶 [`MAX_RETRY_AFTER`]，交互场景不盲睡）。
/// 仅重试「发起请求」阶段；流式开始后的中断不重试，避免工具副作用重复执行。
async fn post_with_retry(
    client: &reqwest::Client,
    url: &str,
    body: &serde_json::Value,
    api_key: &str,
) -> Result<reqwest::Response, HarnessError> {
    const MAX_RETRIES: u32 = 2;
    const BASE_DELAY: Duration = Duration::from_millis(800);
    /// `Retry-After` 的等待上限：这是交互式 harness，不该被要求睡到天荒地老。
    const MAX_RETRY_AFTER: Duration = Duration::from_secs(10);

    let mut attempt: u32 = 0;
    loop {
        attempt += 1;
        let sent = client
            .post(url)
            .bearer_auth(api_key)
            .json(body)
            .send()
            .await;

        match sent {
            Ok(resp) if resp.status().is_success() => return Ok(resp),
            Ok(resp) if is_retryable_status(resp.status()) && attempt <= MAX_RETRIES => {
                // 429 时服务端通常明确告知何时可重试；没有就退回指数退避。
                let delay = retry_after_delay(resp.headers())
                    .map(|d| d.min(MAX_RETRY_AFTER))
                    .unwrap_or_else(|| BASE_DELAY * 2u32.pow(attempt - 1));
                tokio::time::sleep(delay).await;
            }
            Ok(resp) => {
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                let preview: String = text.chars().take(500).collect();
                return Err(HarnessError::llm(format!("status {status}: {preview}")));
            }
            Err(e) if attempt <= MAX_RETRIES && (e.is_connect() || e.is_timeout()) => {
                tokio::time::sleep(BASE_DELAY * 2u32.pow(attempt - 1)).await;
            }
            Err(e) => return Err(HarnessError::llm(format!("请求失败: {e}"))),
        }
    }
}

fn is_retryable_status(status: reqwest::StatusCode) -> bool {
    status.as_u16() == 429 || status.is_server_error()
}

/// 解析 `Retry-After` 头：支持「秒数」与 HTTP-date（RFC 2822）两种格式；
/// 缺失、非法或指向过去时返回 `None`（调用方退回指数退避）。
fn retry_after_delay(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    let v = headers.get(reqwest::header::RETRY_AFTER)?.to_str().ok()?;
    let v = v.trim();
    if let Ok(secs) = v.parse::<u64>() {
        return Some(Duration::from_secs(secs));
    }
    let t = chrono::DateTime::parse_from_rfc2822(v).ok()?;
    (t.with_timezone(&chrono::Utc) - chrono::Utc::now())
        .to_std()
        .ok()
        .map(|d| d + Duration::from_secs(1))
}

/// 处理一行 SSE；`response.completed` 返回 `Some(Completion)`，失败事件转错误，其余转发增量。
fn handle_event<F: FnMut(&str)>(
    line: &str,
    on_token: &mut F,
) -> Result<Option<Completion>, HarnessError> {
    let Some(data) = line.strip_prefix("data: ") else {
        return Ok(None);
    };
    let data = data.trim();
    if data == "[DONE]" || data.is_empty() {
        return Ok(None);
    }
    let Ok(v) = serde_json::from_str::<Value>(data) else {
        return Ok(None);
    };

    let etype = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
    match etype {
        "response.output_text.delta" => {
            if let Some(delta) = v.get("delta").and_then(|d| d.as_str()) {
                on_token(delta);
            }
            Ok(None)
        }
        "response.completed" | "response.done" => {
            let payload = v.get("response").cloned().unwrap_or_else(|| v.clone());
            let resp: Response = serde_json::from_value(payload)
                .map_err(|e| HarnessError::llm(format!("解析响应失败: {e}")))?;
            if let Some(err) = &resp.error {
                return Err(HarnessError::llm(format!("服务端错误: {}", err.message)));
            }
            Ok(Some(Completion::from_response(&resp)))
        }
        "response.failed" => {
            let msg = v
                .pointer("/response/status_details/error/message")
                .or_else(|| v.pointer("/response/error/message"))
                .and_then(|m| m.as_str())
                .unwrap_or("unknown failure");
            Err(HarnessError::llm(format!("生成失败: {msg}")))
        }
        "error" => {
            let msg = v
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("unknown error");
            Err(HarnessError::llm(format!("事件错误: {msg}")))
        }
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 预置取消信号：complete_once 应立即返回 Cancelled，不发起任何网络请求。
    #[tokio::test]
    async fn complete_once_honours_preset_cancel() {
        let cfg = Config {
            model: "m".to_string(),
            // cancel 在触碰网络前检查，此地址不会被请求到。
            api_base: "http://127.0.0.1:1".to_string(),
            api_key: "k".to_string(),
            max_iterations: 1,
            policy: crate::sandbox::SandboxPolicy {
                enabled: false,
                allow_network: false,
                writable_roots: vec![],
                cwd: None,
            },
            sandbox_dir: std::path::PathBuf::from("."),
            stateful: false,
            force_tools: false,
            approve_tools: false,
            multi_agent: false,
            codegen: true,
            codegen_model: None,
            max_tokens: 0,
            history_max_items: 200,
            codegen_plugin_dir: None,
            theme: "dark".to_string(),
            mcp_servers: vec![],
        };
        let cancel = AtomicBool::new(true);
        let err = complete_once(&cfg, "m", "s", "p", &cancel)
            .await
            .unwrap_err();
        assert!(matches!(err, HarnessError::Cancelled));
    }

    // 漂移守卫：基础系统提示里的工具清单必须覆盖 builtin_tools() 的全部工具名，
    // 否则新增工具时模型不会被告知。
    #[test]
    fn base_instructions_covers_all_builtin_tools() {
        let tools = crate::tools::builtin_tools();
        let s = base_instructions(&tools);
        assert!(!tools.is_empty(), "应有至少一个内置工具");
        for t in &tools {
            assert!(s.contains(t.name.as_str()), "系统提示应提及工具 {}", t.name);
        }
    }

    // Retry-After 解析：秒数 / HTTP-date / 缺失三种情况。
    #[test]
    fn retry_after_seconds_date_and_missing() {
        use reqwest::header::{HeaderMap, HeaderValue, RETRY_AFTER};

        let mut h = HeaderMap::new();
        h.insert(RETRY_AFTER, HeaderValue::from_static("3"));
        assert_eq!(retry_after_delay(&h), Some(Duration::from_secs(3)));

        // 未来 60 秒的 HTTP-date：等待时长应在 (50, 61] 秒区间。
        let future = chrono::Utc::now() + chrono::Duration::seconds(60);
        let v = future.to_rfc2822();
        h.insert(RETRY_AFTER, HeaderValue::from_str(&v).unwrap());
        let d = retry_after_delay(&h).unwrap();
        assert!(d > Duration::from_secs(50) && d <= Duration::from_secs(61));

        // 过去的 date 与缺失头：都应返回 None（退回指数退避）。
        let past = chrono::Utc::now() - chrono::Duration::seconds(30);
        let v = past.to_rfc2822();
        h.insert(RETRY_AFTER, HeaderValue::from_str(&v).unwrap());
        assert_eq!(retry_after_delay(&h), None);
        assert_eq!(retry_after_delay(&HeaderMap::new()), None);
    }

    // SSE 增量与 completed 事件：delta 转发回调，completed 抽出 Completion。
    #[test]
    fn parses_delta_then_completed() {
        let mut tokens = String::new();
        let delta = format!(
            "data: {}",
            json!({"type":"response.output_text.delta","delta":"he"})
        );
        assert!(
            handle_event(&delta, &mut |d| tokens.push_str(d))
                .unwrap()
                .is_none()
        );

        let done = format!(
            "data: {}",
            json!({"type":"response.completed","response":{
                "id":"r1",
                "output":[{"type":"message","content":[{"type":"output_text","text":"hello"}]}]
            }})
        );
        let completion = handle_event(&done, &mut |d| tokens.push_str(d))
            .unwrap()
            .expect("completed should yield completion");
        assert_eq!(completion.text.as_deref(), Some("hello"));
        assert_eq!(tokens, "he");
    }

    // 失败事件应转为错误；非 data 行 / [DONE] 应被忽略。
    #[test]
    fn failed_event_is_error_and_noise_ignored() {
        let failed = format!(
            "data: {}",
            json!({"type":"response.failed","response":{"error":{"message":"boom"}}})
        );
        assert!(handle_event(&failed, &mut |_| {}).is_err());

        assert!(handle_event(": keep-alive", &mut |_| {}).unwrap().is_none());
        assert!(handle_event("data: [DONE]", &mut |_| {}).unwrap().is_none());
    }

    // 跨网络分包的多字节字符不应变成乱码：按 \n 切分后再解码。
    #[test]
    fn take_line_decodes_multibyte_split_across_chunks() {
        let mut buf: Vec<u8> = Vec::new();
        // "你好么" 的 UTF-8 字节，模拟「么」被切在两个 chunk 中间。
        let full = "data: 你好么\n".as_bytes().to_vec();
        let split_at = full.len() - 2; // 「么」(3 字节) 的中间
        buf.extend_from_slice(&full[..split_at]);
        assert!(take_line(&mut buf).is_none(), "不完整的行不应提前解码");

        buf.extend_from_slice(&full[split_at..]);
        let line = take_line(&mut buf).expect("补齐后应取出完整行");
        assert_eq!(line, "data: 你好么", "跨 chunk 的中文不应出现 U+FFFD");
        assert!(!line.contains('\u{FFFD}'));

        // 连续多行 + 残留不完整字节留在缓冲区。
        buf.clear();
        buf.extend_from_slice(b"a\nb\nc");
        assert_eq!(take_line(&mut buf).as_deref(), Some("a"));
        assert_eq!(take_line(&mut buf).as_deref(), Some("b"));
        assert!(take_line(&mut buf).is_none(), "残留的 c 应留在缓冲区");
    }
}
