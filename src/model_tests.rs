use crate::model::{Completion, Response};

#[test]
fn ignores_reasoning_and_extracts_message_and_tool() {
    let json = serde_json::json!({
        "id": "r1",
        "output": [
            { "type": "reasoning", "summary": [ { "type": "summary_text", "text": "thinking" } ] },
            { "type": "message", "content": [ { "type": "output_text", "text": "hi" } ] },
            { "type": "function_call", "call_id": "c1", "name": "shell", "arguments": "{\"command\":\"ls\"}" }
        ]
    });
    let response: Response = serde_json::from_value(json).unwrap();
    let completion = Completion::from_response(&response);
    assert_eq!(completion.text.as_deref(), Some("hi"));
    assert_eq!(completion.function_calls.len(), 1);
    assert_eq!(completion.function_calls[0].name, "shell");
    assert_eq!(completion.reasoning.as_deref(), Some("thinking"));
}

#[test]
fn reasoning_only_yields_no_text() {
    let json = serde_json::json!({
        "id": "r2",
        "output": [ { "type": "reasoning", "text": "hm" } ]
    });
    let response: Response = serde_json::from_value(json).unwrap();
    let completion = Completion::from_response(&response);
    assert!(completion.text.is_none());
    assert!(completion.reasoning.is_some());
}

#[test]
fn extracts_reasoning_summary() {
    let json = serde_json::json!({
        "id": "r3",
        "output": [ { "type": "reasoning", "summary": [ { "type": "summary_text", "text": "step" } ] } ]
    });
    let response: Response = serde_json::from_value(json).unwrap();
    let completion = Completion::from_response(&response);
    assert_eq!(completion.reasoning.as_deref(), Some("step"));
}

#[test]
fn missing_usage_defaults_to_zero() {
    let json = serde_json::json!({ "id": "r4", "output": [] });
    let response: Response = serde_json::from_value(json).unwrap();
    let completion = Completion::from_response(&response);
    assert_eq!(completion.usage.input_tokens, 0);
    assert_eq!(completion.usage.output_tokens, 0);
}
