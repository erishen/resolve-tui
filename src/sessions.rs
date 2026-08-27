use std::fs;
use std::path::{Path, PathBuf};

/// 会话存储目录（相对或绝对皆可），可被 `HARNESS_SESSIONS_DIR` 覆盖。
/// 存放若干 `<name>.json`，每个文件是一段对话历史。
pub fn sessions_dir() -> PathBuf {
    match std::env::var("HARNESS_SESSIONS_DIR") {
        Ok(v) if !v.trim().is_empty() => PathBuf::from(v.trim()),
        _ => PathBuf::from(".resolve-tui-sessions"),
    }
}

/// 自动生成的会话名（按 Unix 时间戳），用于 `/create` 不带参数时。
pub fn auto_name() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("session-{secs}")
}

/// 单个会话的元信息（用于 list 展示）。
#[derive(Debug, Clone)]
pub struct SessionMeta {
    /// 列表中的序号（从 0 开始，最新在前），可作为 resume 的 key。
    pub index: usize,
    /// 会话名（文件名去后缀）。
    pub name: String,
    /// 完整路径。
    pub path: PathBuf,
    /// 修改时间（本地时区，YYYY-MM-DD HH:MM）。
    pub modified: String,
    /// 首条用户消息预览（最多 60 字）。
    pub preview: String,
}

/// 列出会话目录下的所有会话，按修改时间倒序（最新在前），并附上序号。
pub fn list(dir: &Path) -> Vec<SessionMeta> {
    let mut out = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let name = p
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            let modified = e
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .map(|t| {
                    chrono::DateTime::<chrono::Local>::from(t)
                        .format("%Y-%m-%d %H:%M")
                        .to_string()
                })
                .unwrap_or_default();
            let preview = read_preview(&p);
            out.push(SessionMeta {
                index: 0,
                name,
                path: p,
                modified,
                preview,
            });
        }
    }
    out.sort_by(|a, b| b.modified.cmp(&a.modified));
    for (i, s) in out.iter_mut().enumerate() {
        s.index = i;
    }
    out
}

/// 把 `key`（序号或名称）解析为具体的会话文件路径；找不到返回 `None`。
/// - 纯数字 → 按列表序号匹配
/// - 其它 → 当作名称，在会话目录下找 `<key>.json`
pub fn resolve(dir: &Path, key: &str) -> Option<PathBuf> {
    if let Ok(n) = key.parse::<usize>() {
        return list(dir).into_iter().find(|s| s.index == n).map(|s| s.path);
    }
    let p = dir.join(format!("{key}.json"));
    if p.exists() { Some(p) } else { None }
}

/// 删除会话文件（按序号或名称），返回被删路径；找不到或删除失败返回 `None`。
pub fn delete(dir: &Path, key: &str) -> Option<PathBuf> {
    let path = resolve(dir, key)?;
    std::fs::remove_file(&path).ok()?;
    Some(path)
}

/// 读取首条用户消息文本作为预览（只解析 JSON，不依赖 `InputItem`）。
/// 兼容新格式 `{messages:[...]}` 与旧版裸数组 `[...]`。
fn read_preview(p: &Path) -> String {
    if let Ok(data) = fs::read_to_string(p)
        && let Ok(value) = serde_json::from_str::<serde_json::Value>(&data)
    {
        let items = match &value {
            serde_json::Value::Array(a) => a.clone(),
            serde_json::Value::Object(_) => value
                .get("messages")
                .and_then(|m| m.as_array())
                .cloned()
                .unwrap_or_default(),
            _ => return String::new(),
        };
        for item in &items {
            if item.get("type").and_then(|t| t.as_str()) == Some("message")
                && let Some(content) = item.get("content").and_then(|c| c.as_array())
            {
                for part in content {
                    if let Some(t) = part.get("text").and_then(|t| t.as_str()) {
                        return t.chars().take(60).collect();
                    }
                }
            }
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_reads_saved_session_files() {
        let dir = std::env::temp_dir().join("harness_sessions_test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        // 写两个会话文件，含一条 user 消息作为预览来源。
        fs::write(
            dir.join("alpha.json"),
            r#"[{"type":"message","role":"user","content":[{"type":"input_text","text":"你好世界"}]}]"#,
        )
        .unwrap();
        fs::write(
            dir.join("beta.json"),
            r#"[{"type":"message","role":"user","content":[{"type":"input_text","text":"另一个会话"}]}]"#,
        )
        .unwrap();

        let sessions = list(&dir);
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].index, 0);
        assert_eq!(sessions[1].index, 1);

        let by_index = resolve(&dir, "0").expect("resolve 0");
        assert!(by_index.ends_with("alpha.json") || by_index.ends_with("beta.json"));
        let by_name = resolve(&dir, "beta").expect("resolve beta");
        assert!(by_name.ends_with("beta.json"));
        assert!(resolve(&dir, "nope").is_none());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn delete_removes_named_session() {
        let dir = std::env::temp_dir().join("harness_sessions_del_test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("gone.json"), "[]").unwrap();

        let deleted = delete(&dir, "gone").expect("should delete");
        assert!(deleted.ends_with("gone.json"));
        assert!(!dir.join("gone.json").exists());
        // 再删一次应返回 None。
        assert!(delete(&dir, "gone").is_none());

        let _ = fs::remove_dir_all(&dir);
    }
}
