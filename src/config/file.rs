//! 配置文件读写：定位 config.toml、解析 TOML、增删 `[mcp_servers.<name>]` 段。
//!
//! 增删采用行级文本编辑（而非整体重写），以保留用户手工配置里的注释与排版。

use std::path::{Path, PathBuf};

use crate::{HarnessError, config::TomlConfig};

/// 配置文件路径：`$HARNESS_CONFIG` 优先，否则 `<config_dir>/resolve-tui/config.toml`。
/// 文件不存在时也返回路径（调用方按需创建），无法确定目录时返回 `None`。
pub fn config_file() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("HARNESS_CONFIG")
        && !p.trim().is_empty()
    {
        return Some(PathBuf::from(p.trim()));
    }
    dirs::config_dir().map(|d| d.join("resolve-tui").join("config.toml"))
}

/// 向配置文件追加一个 `[mcp_servers.<name>]` 段（`/mcp add` 的持久化）。
/// 保留原文件的注释与排版：只做文本追加，不整体重写。
/// 同名段已存在时返回错误（避免静默覆盖手工配置）。
#[cfg_attr(not(feature = "tui"), allow(dead_code))]
pub fn append_mcp_server(
    path: &Path,
    name: &str,
    command: &str,
    args: &[String],
) -> Result<(), HarnessError> {
    if !valid_server_name(name) {
        return Err(HarnessError::config(format!(
            "server 名只能含字母/数字/-/_，实际: {name}"
        )));
    }
    let header = format!("[mcp_servers.{name}]");
    let existing = std::fs::read_to_string(path).unwrap_or_default();
    for line in existing.lines() {
        if line.trim() == header {
            return Err(HarnessError::config(format!(
                "config.toml 中已存在 {header}，请手动编辑或先 /mcp remove"
            )));
        }
    }
    // TOML basic string 转义与 JSON 基本一致（\ " 与控制字符）。
    let esc = |s: &str| {
        let mut out = String::with_capacity(s.len());
        for c in s.chars() {
            match c {
                '\\' => out.push_str("\\\\"),
                '"' => out.push_str("\\\""),
                c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04X}", c as u32)),
                c => out.push(c),
            }
        }
        out
    };
    let mut block = String::new();
    if !existing.is_empty() && !existing.ends_with('\n') {
        block.push('\n');
    }
    block.push_str(&format!("{header}\ncommand = \"{}\"\n", esc(command)));
    if !args.is_empty() {
        let list: Vec<String> = args.iter().map(|a| format!("\"{}\"", esc(a))).collect();
        block.push_str(&format!("args = [{}]\n", list.join(", ")));
    }
    std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")))
        .map_err(|e| HarnessError::config(format!("创建配置目录失败: {e}")))?;
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut f| std::io::Write::write_all(&mut f, block.as_bytes()))
        .map_err(|e| HarnessError::config(format!("写入 {path:?} 失败: {e}")))
}

/// 从配置文件删除 `[mcp_servers.<name>]` 段（`/mcp remove` 的持久化）。
/// 同样只做行级删除以保留其余内容。返回是否确有删除。
#[cfg_attr(not(feature = "tui"), allow(dead_code))]
pub fn remove_mcp_server(path: &Path, name: &str) -> Result<bool, HarnessError> {
    if !valid_server_name(name) {
        return Err(HarnessError::config(format!("server 名非法: {name}")));
    }
    let existing = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(_) => return Ok(false),
    };
    let header = format!("[mcp_servers.{name}]");
    let mut out = String::with_capacity(existing.len());
    let mut skipping = false;
    let mut removed = false;
    for line in existing.lines() {
        let t = line.trim();
        if t == header {
            skipping = true;
            removed = true;
            continue;
        }
        if skipping {
            // 下一个顶层表头出现则停止跳过。
            if t.starts_with('[') && t.ends_with(']') {
                skipping = false;
            } else {
                continue;
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    if removed {
        std::fs::write(path, out)
            .map_err(|e| HarnessError::config(format!("写入 {path:?} 失败: {e}")))?;
    }
    Ok(removed)
}

/// server 名约束：TOML 表键直接内插，限制为安全字符。
fn valid_server_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// 读取并解析 TOML 配置文件；解析失败时打印告警并忽略（不阻断启动）。
pub(crate) fn read_toml(path: &Path) -> Option<TomlConfig> {
    let data = std::fs::read_to_string(path).ok()?;
    match toml::from_str::<TomlConfig>(&data) {
        Ok(cfg) => Some(cfg),
        Err(e) => {
            eprintln!("[config] 解析 {path:?} 失败，已忽略：{e}");
            None
        }
    }
}
