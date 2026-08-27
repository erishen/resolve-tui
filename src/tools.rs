use serde_json::Value;

use crate::{
    HarnessError,
    model::ResponseTool,
    sandbox::{self, SandboxPolicy},
};

/// 暴露给模型的内置工具集。
pub fn builtin_tools() -> Vec<ResponseTool> {
    fn tool(name: &str, description: &str, properties: Value, required: &[&str]) -> ResponseTool {
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

    vec![
        tool(
            "shell",
            "在沙箱中执行 shell 命令，返回 stdout/stderr。",
            serde_json::json!({
                "command": { "type": "string", "description": "要执行的命令" }
            }),
            &["command"],
        ),
        tool(
            "read_file",
            "读取文本文件内容。",
            serde_json::json!({
                "path": { "type": "string", "description": "文件路径" }
            }),
            &["path"],
        ),
        tool(
            "write_file",
            "写入（覆盖）文本文件。路径基于当前沙箱工作区（见系统提示 [沙箱]），相对路径即以工作区为准；工作区外不可写。",
            serde_json::json!({
                "path": { "type": "string", "description": "文件路径（相对当前工作区）" },
                "content": { "type": "string", "description": "完整文件内容" }
            }),
            &["path", "content"],
        ),
        tool(
            "list_dir",
            "列出目录下的条目。",
            serde_json::json!({
                "path": { "type": "string", "description": "目录路径" }
            }),
            &["path"],
        ),
    ]
}

/// 单次工具输出的字符上限：超出后保留头尾，防止一条 `cat 大文件` 撑爆模型上下文。
const MAX_TOOL_CHARS: usize = 8 * 1024;

/// 执行一次工具调用，返回给模型的输出文本。
pub async fn execute(
    name: &str,
    arguments: &str,
    policy: &SandboxPolicy,
) -> Result<String, HarnessError> {
    let args: Value = serde_json::from_str(arguments.trim()).unwrap_or(Value::Null);

    let output = match name {
        "shell" => {
            let command = str_arg(&args, "command")?.to_string();
            sandbox::run_command("sh", ["-c", command.as_str()].as_slice(), policy).await?
        }
        "read_file" => {
            let path = str_arg(&args, "path")?;
            let path = policy.resolve_read(std::path::Path::new(path));
            if !policy.is_readable(&path) {
                return Err(HarnessError::tool(format!("路径不在可读范围: {path:?}")));
            }
            std::fs::read_to_string(&path)
                .map_err(|e| HarnessError::tool(format!("读取失败: {e}")))?
        }
        "write_file" => {
            let raw = str_arg(&args, "path")?;
            let path = policy.resolve(std::path::Path::new(raw));
            let content = str_arg(&args, "content")?;
            if !policy.is_writable(&path) {
                return Err(HarnessError::tool(format!("路径不在可写白名单: {path:?}")));
            }
            let bytes = content.len();
            std::fs::write(&path, content)
                .map_err(|e| HarnessError::tool(format!("写入失败: {e}")))?;
            format!("已写入 {}（{bytes} 字节）", path.display())
        }
        "list_dir" => {
            let path = str_arg(&args, "path")?;
            let path = policy.resolve_read(std::path::Path::new(path));
            if !policy.is_readable(&path) {
                return Err(HarnessError::tool(format!("路径不在可读范围: {path:?}")));
            }
            let mut entries = Vec::new();
            let rd = std::fs::read_dir(&path)
                .map_err(|e| HarnessError::tool(format!("列目录失败: {e}")))?;
            for e in rd {
                let e = e.map_err(|err| HarnessError::tool(format!("列目录失败: {err}")))?;
                let name = e.file_name().to_string_lossy().to_string();
                if e.path().is_dir() {
                    entries.push(format!("{name}/"));
                } else {
                    entries.push(name);
                }
            }
            entries.sort();
            if entries.is_empty() {
                "（空目录）".to_string()
            } else {
                entries.join("\n")
            }
        }
        other => return Err(HarnessError::tool(format!("未知工具: {other}"))),
    };
    Ok(truncate_output(output))
}

/// 截断超长输出：保留头尾各一半，中间以标记说明（按字符切，避免切断多字节中文）。
pub(crate) fn truncate_output(s: String) -> String {
    let total = s.chars().count();
    if total <= MAX_TOOL_CHARS {
        return s;
    }
    let keep = MAX_TOOL_CHARS / 2;
    let head: String = s.chars().take(keep).collect();
    let tail: String = s.chars().skip(total - keep).collect();
    format!("{head}\n…[输出过长已截断：原文约 {total} 字符，中间内容省略]…\n{tail}")
}

fn str_arg<'a>(v: &'a Value, key: &str) -> Result<&'a str, HarnessError> {
    v.get(key)
        .and_then(|x| x.as_str())
        .ok_or_else(|| HarnessError::tool(format!("缺少参数 {key}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_keeps_head_and_tail_with_marker() {
        let big = format!(
            "{}{}{}",
            "a".repeat(MAX_TOOL_CHARS),
            "MIDDLE_MARKER",
            "z".repeat(MAX_TOOL_CHARS)
        );
        let out = truncate_output(big.clone());
        assert!(out.chars().count() < big.chars().count());
        assert!(out.contains("已截断"), "应包含截断标记");
        assert!(out.starts_with('a'), "应保留头部");
        assert!(out.ends_with('z'), "应保留尾部");
        assert!(!out.contains("MIDDLE_MARKER"), "中间内容应被省略");
    }

    #[test]
    fn short_output_untouched() {
        let s = "hello 你好".to_string();
        assert_eq!(truncate_output(s.clone()), s);
    }
}
