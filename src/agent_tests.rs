use crate::agent::Conversation;
use crate::model::{Completion, FunctionCall, InputItem};

// 触发编译器确认公开 API 形态稳定。
#[test]
fn conversation_new_is_usable() {
    let mut conv = Conversation::new();
    // 无 previous_id 初始状态。
    let _ = &mut conv;
}

// 仅验证 Completion 能正确抽取 id（配合 previous_response_id 续接上下文）。
#[test]
fn completion_captures_response_id() {
    let json = serde_json::json!({
        "id": "resp_123",
        "output": [ { "type": "message", "content": [ { "type": "output_text", "text": "hi" } ] } ]
    });
    let response: crate::model::Response = serde_json::from_value(json).unwrap();
    let completion = crate::model::Completion::from_response(&response);
    assert_eq!(completion.id.as_deref(), Some("resp_123"));
    assert_eq!(completion.text.as_deref(), Some("hi"));
}

// 会话持久化往返：save 再 load 应能还原相同的 input 历史。
#[test]
fn session_save_load_roundtrip() {
    let mut conv = Conversation::new();
    conv.input_mut().push(InputItem::message("user", "你好"));
    conv.input_mut().push(InputItem::function_call_output(
        "call_1".to_string(),
        "结果".to_string(),
    ));

    let path = std::env::temp_dir().join("resolve_tui_session_test.json");
    let path = path.to_str().unwrap();
    conv.save(path).expect("save failed");
    conv.clear();

    conv.load(path).expect("load failed");
    let json: serde_json::Value = serde_json::to_value(conv.input()).unwrap();
    assert_eq!(json[0]["type"], "message");
    assert_eq!(json[0]["role"], "user");
    assert_eq!(json[0]["content"][0]["text"], "你好");
    assert_eq!(json[1]["type"], "function_call_output");

    let _ = std::fs::remove_file(path);
}

/// 会话含敏感对话，落盘必须以 0600 权限写入，防止同机其他用户读取。
#[cfg(unix)]
#[test]
fn session_save_writes_0600_permissions() {
    use std::os::unix::fs::PermissionsExt as _;

    let path = std::env::temp_dir().join("resolve_tui_session_perm_test.json");
    let path = path.to_str().unwrap();
    let _ = std::fs::remove_file(path);

    let conv = Conversation::new();
    conv.save(path).expect("save failed");

    let mode = std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "会话文件权限应为 0600，实际 {mode:o}");
    let _ = std::fs::remove_file(path);
}

// 会话续接应保留累计 token 用量，使预算控制从「已消耗」处继续计数（item 12）。
#[test]
fn session_load_preserves_total_tokens() {
    let dir = std::env::temp_dir().join("resolve_tui_session_tok_test.json");
    let path = dir.to_str().unwrap();
    // 新格式：含 total_tokens 与 messages。
    fs_write_json(
        path,
        serde_json::json!({
            "version": 1u32,
            "total_tokens": 42u64,
            "messages": [{"type":"message","role":"user","content":[{"type":"input_text","text":"续接"}]}]
        }),
    );

    let mut conv = Conversation::new();
    conv.load(path).expect("load failed");
    assert_eq!(conv.total_tokens(), 42, "累计用量应从会话恢复");
    assert_eq!(conv.input().len(), 1);

    let _ = std::fs::remove_file(path);
}

// 旧版裸数组会话可兼容加载，total_tokens 视为 0。
#[test]
fn session_load_legacy_array_keeps_total_zero() {
    let dir = std::env::temp_dir().join("resolve_tui_session_legacy_test.json");
    let path = dir.to_str().unwrap();
    fs_write_json(
        path,
        serde_json::json!([{"type":"message","role":"user","content":[{"type":"input_text","text":"旧格式"}]}]),
    );

    let mut conv = Conversation::new();
    conv.load(path).expect("legacy load failed");
    assert_eq!(conv.total_tokens(), 0, "旧格式无累计用量");
    assert_eq!(conv.input().len(), 1);

    let _ = std::fs::remove_file(path);
}

// 回归：function_call 的 arguments 必须序列化为 JSON 字符串（而非对象），
// 否则上游 Responses API 会 400（untagged enum ResponseInput 不匹配）。
#[test]
fn function_call_arguments_serialized_as_string() {
    let mut conv = Conversation::new();
    conv.input_mut().push(InputItem::function_call(
        "call_1".to_string(),
        "shell".to_string(),
        "{\"command\":\"ls -la\"}".to_string(),
        "fc_1".to_string(),
    ));

    let path = std::env::temp_dir().join("resolve_tui_fc_args_test.json");
    let path = path.to_str().unwrap();
    conv.save(path).expect("save failed");

    let raw = std::fs::read_to_string(path).unwrap();
    let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let fc = json["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["type"] == "function_call")
        .expect("存档应包含 function_call");
    assert!(
        fc["arguments"].is_string(),
        "arguments 必须为 JSON 字符串，实际: {}",
        fc["arguments"]
    );
    assert_eq!(
        fc["id"], "fc_1",
        "回灌的 function_call 必须携带条目 id（部分网关强制要求）"
    );

    let mut conv2 = Conversation::new();
    conv2.load(path).expect("load failed");
    let item = conv2
        .input()
        .iter()
        .find(|i| matches!(i, InputItem::FunctionCall { .. }))
        .unwrap();
    if let InputItem::FunctionCall { arguments, .. } = item {
        assert_eq!(arguments, "{\"command\":\"ls -la\"}");
    }

    let _ = std::fs::remove_file(path);
}

// 回归：旧存档的 function_call 没有 id 字段也能载入（serde default），
// 且构造回灌项时空 id 会以 call_id 兜底，保证发给网关的历史里 id 始终非空。
#[test]
fn legacy_function_call_without_id_gets_call_id_fallback() {
    let raw = r#"{"type":"function_call","call_id":"call_old","name":"shell","arguments":"{}"}"#;
    let item: InputItem = serde_json::from_str(raw).expect("旧存档应可反序列化");

    // 直接从旧数据来的项 id 为空（仅存档兼容场景）；经 function_call 构造器重建后必须非空。
    if let InputItem::FunctionCall { call_id, id, .. } = &item {
        assert_eq!(call_id, "call_old");
        assert!(id.is_empty(), "旧存档载入时 id 应为空，实际: {id}");
    } else {
        panic!("expected FunctionCall");
    }

    let rebuilt = InputItem::function_call(
        "call_old".to_string(),
        "shell".to_string(),
        "{}".to_string(),
        String::new(),
    );
    assert_eq!(
        rebuilt.function_call_id(),
        Some("call_old"),
        "空 id 应回退到 call_id"
    );

    let json = serde_json::to_value(&rebuilt).unwrap();
    assert_eq!(json["id"], "call_old", "序列化后的 id 必须非空");
}

// 回归：无状态模式下 `accumulate` 必须把助手纯文本回复回灌到本地历史，
// 否则下一轮发回去的 input 缺少 assistant 消息，上游会 400。
#[test]
fn accumulate_stores_assistant_reply_in_nonstateful() {
    let mut conv = Conversation::new();

    let text_completion = Completion {
        id: None,
        function_calls: vec![],
        text: Some("我在你的电脑上运行".to_string()),
        reasoning: None,
        usage: Default::default(),
    };
    conv.accumulate(&text_completion);
    let input = conv.input();
    assert_eq!(input.len(), 1, "纯文本回复应产生一条 assistant 消息");
    let msg = &input[0];
    assert!(
        matches!(msg, InputItem::Message { role, .. } if role.as_str() == "assistant"),
        "应为 assistant 消息，实际: {:?}",
        input[0]
    );

    // 工具调用轮不应额外产生 assistant 消息（由 drive 回灌 function_call）。
    let fc_completion = Completion {
        id: None,
        function_calls: vec![FunctionCall {
            call_id: "c1".into(),
            name: "shell".into(),
            arguments: "{}".into(),
            id: "fc_c1".into(),
        }],
        text: None,
        reasoning: None,
        usage: Default::default(),
    };
    conv.accumulate(&fc_completion);
    assert_eq!(
        conv.input().len(),
        1,
        "工具调用轮不应额外产生 assistant 消息"
    );
}

// 工具失败摘要：压成单行 + 截断，供 UI 直接展示 err 原因。
#[test]
fn flatten_error_single_line_and_truncated() {
    let multi = "line1\nline2\r\n  line3";
    assert_eq!(super::flatten_error(multi), "line1 line2 line3");
    let long = "错".repeat(200);
    let out = super::flatten_error(&long);
    assert_eq!(out.chars().count(), 161, "160 字符 + 省略号");
    assert!(out.ends_with('…'));
}

fn fs_write_json(path: &str, value: serde_json::Value) {
    let data = serde_json::to_string_pretty(&value).unwrap();
    std::fs::write(path, data).unwrap();
}

// /mcp add|remove：运行时挂载后远端工具立即并入请求列表；摘除后清空；坏命令不影响已有状态。
#[tokio::test]
async fn add_and_remove_mcp_server_at_runtime() {
    use crate::mcp::McpManager;
    use std::collections::HashMap;

    // 与 mcp.rs 相同的假 server：$1 为工具名。
    let dir = std::env::temp_dir().join(format!("harness_agent_mcp_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("fake.sh");
    std::fs::write(
        &script,
        r#"#!/bin/sh
TOOL="$1"
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"capabilities":{"tools":{}}}}\n' "$id" ;;
    *'"method":"tools/list"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"%s","description":"d","inputSchema":{"type":"object"}}]}}\n' "$id" "$TOOL" ;;
    *'"method":"tools/call"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"ok"}]}}\n' "$id" ;;
  esac
done
"#,
    )
    .unwrap();

    let mut conv = Conversation::new();
    assert!(conv.mcp_status().is_empty());

    // 初始无 manager 时首次 add 也能工作。
    let script_str = script.to_str().unwrap().to_string();
    conv.add_mcp("alpha", "sh", &[script_str.clone(), "tool_a".to_string()])
        .await
        .expect("首次 add 应成功");
    assert_eq!(conv.visible_tools().len(), 5, "4 内置 + 1 远端");
    assert!(conv.mcp_status()[0].contains("alpha"));

    // 再加一个，然后只摘除第一个。
    conv.add_mcp("beta", "sh", &[script_str.clone(), "tool_b".to_string()])
        .await
        .expect("第二次 add 应成功");
    assert_eq!(conv.visible_tools().len(), 6);

    // 重名 add 应失败且不产生重复工具。
    assert!(
        conv.add_mcp("beta", "sh", &[script_str.clone(), "x".to_string()])
            .await
            .is_err()
    );
    assert_eq!(conv.visible_tools().len(), 6);

    conv.remove_mcp("alpha").await.expect("摘除 alpha 应成功");
    assert_eq!(
        conv.visible_tools().len(),
        5,
        "alpha 的工具应被清理，beta 保留"
    );
    assert!(
        conv.visible_tools()
            .iter()
            .any(|(n, _, _)| n == "mcp_beta_tool_b"),
        "beta 工具仍在: {:?}",
        conv.visible_tools()
    );

    // 摘除不存在的名字报错。
    assert!(conv.remove_mcp("nope").await.is_err());

    // 坏命令 add 失败后状态不被污染（manager 可继续用）。
    assert!(
        conv.add_mcp("broken", "/nonexistent-binary-xyz", &[])
            .await
            .is_err()
    );
    assert_eq!(conv.visible_tools().len(), 5);
    conv.add_mcp("gamma", "sh", &[script_str, "tool_c".to_string()])
        .await
        .expect("失败后仍可正常挂载");

    // 防止 unused 警告（McpManager 仅通过 conversation API 使用）。
    drop(McpManager::new());
    let _ = HashMap::<String, String>::new();
    let _ = std::fs::remove_dir_all(&dir);
}

// 历史窗口：未超限时原样返回。
#[test]
fn windowed_history_noop_within_limit() {
    let items = vec![
        InputItem::message("user", "q1"),
        InputItem::message("assistant", "a1"),
    ];
    let w = crate::agent::windowed_history(&items, 10);
    assert_eq!(w.len(), 2);
    // 0 = 不限制。
    assert_eq!(crate::agent::windowed_history(&items, 0).len(), 2);
}

// 超限截断：起点必须落在 user 消息上；function_call/output 配对不被拆散。
#[test]
fn windowed_history_respects_pair_boundaries() {
    let items = vec![
        InputItem::message("user", "q1"),
        InputItem::function_call(
            "c1".into(),
            "shell".into(),
            "{\"command\":\"ls\"}".into(),
            "fc_1".into(),
        ),
        InputItem::function_call_output("c1".into(), "out1".into()),
        InputItem::message("user", "q2"),
        InputItem::message("assistant", "a2"),
    ];
    // 条数窗口恰好落在 q2 上（index 3）：直接从 q2 开始。
    let w = crate::agent::windowed_history(&items, 2);
    assert_eq!(w.len(), 2);
    assert!(matches!(&w[0], InputItem::Message { role, .. } if role == "user"));

    // 条数窗口落在 function_call（index 1）/ output（index 2）中间：
    // 必须向后滑动到下一个 user 边界（q2），绝不从工具对中间开始。
    for max in [3usize, 4] {
        let w = crate::agent::windowed_history(&items, max);
        assert!(matches!(&w[0], InputItem::Message { role, .. } if role == "user"));
        // 窗口必须包含完整尾部（最后两条是 q2 + a2）。
        assert_eq!(
            w.last()
                .and_then(|i| match i {
                    InputItem::Message { content, .. } => content.first().map(|c| c.text.clone()),
                    _ => None,
                })
                .as_deref(),
            Some("a2")
        );
    }
}
