use serde::{Deserialize, Serialize};

/// OpenAI Responses API 的 `input` 项（也是回灌给下一轮的历史项）。
///
/// 与 codex 的 `codex-api` 类似，这里只保留 harness 需要的子集：
/// 用户/系统消息、工具调用结果、以及需要回灌的工具调用本身。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum InputItem {
    #[serde(rename = "message")]
    Message {
        role: String,
        content: Vec<InputContent>,
    },
    #[serde(rename = "function_call_output")]
    FunctionCallOutput {
        #[serde(rename = "call_id")]
        call_id: String,
        output: String,
    },
    /// 模型上一轮产出的工具调用，需作为历史回灌以延续上下文。
    /// 按 OpenAI Responses API 规范，`arguments` 必须是「JSON 字符串」（如 `"{\"command\":\"ls\"}"`），
    /// 不是 JSON 对象；作为对象回灌会触发 400（untagged enum ResponseInput 无法匹配）。
    #[serde(rename = "function_call")]
    FunctionCall {
        #[serde(rename = "call_id")]
        call_id: String,
        name: String,
        /// 反序列化兼容旧存档：若读到的是对象/数组，则取其 JSON 文本；读到字符串则直接使用。
        #[serde(deserialize_with = "de_args_lenient")]
        arguments: String,
        /// 模型返回的条目 `id`（形如 `fc_...`）。部分网关（Agnes 实测）要求回灌时必须携带，
        /// 缺失同样触发 untagged enum 400。旧存档可能没有该字段；为空时以 `call_id` 兜底。
        #[serde(default)]
        id: String,
    },
}

/// 解析 `function_call.arguments`：API 通常以 JSON 字符串给出；
/// 兼容早期以 JSON 对象形式写入的存档，避免加载失败。
fn de_args_lenient<'de, D>(d: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let v = serde_json::Value::deserialize(d)?;
    match v {
        serde_json::Value::String(s) => Ok(s),
        other => Ok(serde_json::to_string(&other).unwrap_or_else(|_| "{}".to_string())),
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InputContent {
    #[serde(rename = "type")]
    pub content_type: String,
    pub text: String,
}

impl InputItem {
    pub fn message(role: &str, text: &str) -> Self {
        Self::Message {
            role: role.to_string(),
            content: vec![InputContent {
                content_type: "input_text".to_string(),
                text: text.to_string(),
            }],
        }
    }

    pub fn function_call_output(call_id: String, output: String) -> Self {
        Self::FunctionCallOutput { call_id, output }
    }

    /// 构造回灌用的 function_call 项；`id` 为空（旧存档 / 网关未返回）时以 `call_id` 兜底，
    /// 保证发给网关的历史里该字段始终非空。
    pub fn function_call(call_id: String, name: String, arguments: String, id: String) -> Self {
        let id = if id.is_empty() { call_id.clone() } else { id };
        Self::FunctionCall {
            call_id,
            name,
            arguments,
            id,
        }
    }

    /// 只读访问条目 id（`fc_...`；测试与调试用）。
    pub fn function_call_id(&self) -> Option<&str> {
        match self {
            Self::FunctionCall { id, .. } => Some(id),
            _ => None,
        }
    }
}

/// 暴露给模型的工具定义（Responses API `tools[]`）。
#[derive(Clone, Debug, Serialize)]
pub struct ResponseTool {
    #[serde(rename = "type")]
    pub tool_type: String,
    pub name: String,
    pub description: String,
    /// JSON Schema 对象，描述 `arguments` 结构。
    pub parameters: serde_json::Value,
}

/// `/responses` 返回的完整响应对象。
///
/// `output` 直接用裸 `Value` 承接：Responses API 可能返回 `message`、`function_call`，
/// 也可能返回 `reasoning` 等其它类型。这里只抽取我们需要的两种，其余忽略，
/// 避免因未知 variant 导致整轮解析失败。
#[derive(Clone, Debug, Deserialize)]
pub struct Response {
    /// 本轮响应 id，作为下一轮 `previous_response_id` 续接上下文。
    #[serde(default)]
    pub id: Option<String>,
    pub output: Vec<serde_json::Value>,
    /// 用量统计（部分网关可能不返回）。
    #[serde(default)]
    pub usage: Option<Usage>,
    #[serde(default)]
    pub error: Option<ResponseError>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ResponseError {
    pub message: String,
}

/// 用量统计（Responses API 的 `usage` 字段）。缺失时归零。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
}

/// 从一轮 Responses 输出里抽取出的、harness 主循环需要的结构。
#[derive(Clone, Debug, Default)]
pub struct Completion {
    pub id: Option<String>,
    pub function_calls: Vec<FunctionCall>,
    pub text: Option<String>,
    /// 推理摘要（reasoning 模型才有），用于 TUI 可折叠展示。
    pub reasoning: Option<String>,
    /// 本轮回应的 token 用量。
    pub usage: Usage,
}

#[derive(Clone, Debug)]
pub struct FunctionCall {
    pub call_id: String,
    pub name: String,
    pub arguments: String,
    /// 模型返回的条目 id（`fc_...`）。回灌历史时必须带上（部分网关强制要求）。
    pub id: String,
}

impl Completion {
    /// 从 `response.output` 中只抽取 `message`（文本）与 `function_call`，忽略其它类型。
    pub fn from_response(response: &Response) -> Self {
        let mut function_calls = Vec::new();
        let mut text = String::new();
        let mut reasoning = String::new();
        for item in &response.output {
            let type_field = item.get("type").and_then(|v| v.as_str());
            match type_field {
                Some("message") => {
                    if let Some(content) = item.get("content").and_then(|c| c.as_array()) {
                        for part in content {
                            if let Some(t) = part.get("text").and_then(|v| v.as_str()) {
                                text.push_str(t);
                            }
                        }
                    }
                }
                Some("function_call") => {
                    let call_id = item
                        .get("call_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let name = item
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let arguments = item
                        .get("arguments")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    // 条目 id（fc_...）：部分网关回灌时强制要求；缺失则以 call_id 兜底。
                    let id = item
                        .get("id")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                        .unwrap_or(&call_id)
                        .to_string();
                    function_calls.push(FunctionCall {
                        call_id,
                        name,
                        arguments,
                        id,
                    });
                }
                Some("reasoning") => {
                    if let Some(r) = extract_reasoning_text(item) {
                        if !reasoning.is_empty() {
                            reasoning.push('\n');
                        }
                        reasoning.push_str(&r);
                    }
                }
                // 其它类型：忽略
                _ => {}
            }
        }
        let text = if text.trim().is_empty() {
            None
        } else {
            Some(text)
        };
        let reasoning = if reasoning.trim().is_empty() {
            None
        } else {
            Some(reasoning)
        };
        Self {
            id: response.id.clone(),
            function_calls,
            text,
            reasoning,
            usage: response.usage.clone().unwrap_or_default(),
        }
    }
}

/// 从 `reasoning` 项中抽取文本：兼容 `text` 直出、`summary[]`/`content[]` 内嵌 `text` 两种形态。
fn extract_reasoning_text(item: &serde_json::Value) -> Option<String> {
    if let Some(t) = item.get("text").and_then(|v| v.as_str()) {
        return Some(t.to_string());
    }
    for key in ["summary", "content"] {
        if let Some(arr) = item.get(key).and_then(|v| v.as_array()) {
            let mut s = String::new();
            for part in arr {
                if let Some(t) = part.get("text").and_then(|v| v.as_str()) {
                    s.push_str(t);
                } else if let Some(t) = part.as_str() {
                    s.push_str(t);
                }
            }
            if !s.is_empty() {
                return Some(s);
            }
        }
    }
    None
}

#[cfg(test)]
#[path = "model_tests.rs"]
mod tests;
