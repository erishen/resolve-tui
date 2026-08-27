//! 插件持久化：列出目录内插件（含触发语与命中统计）、删除、保存经编译校验的检测器。

use std::path::Path;

use crate::{HarnessError, codegen::engine};

use super::cache::{invalidate_cache, load_stats};
use super::{mtime_secs, plugin_name};

/// 插件元信息（供管理界面展示）。
#[derive(Debug, Clone)]
pub struct PluginMeta {
    pub name: String,
    pub trigger: String,
    pub source: String,
    pub mtime: i64,
    pub size: u64,
    /// 累计命中次数（来自 plugins.json；无记录为 0）。
    pub hits: u64,
    /// 最后命中的 Unix 时间戳（秒）；从未命中为 0。
    pub last_hit: i64,
}

/// 列出目录下所有插件（含源码、触发语与命中统计）。
pub fn list_plugins(dir: &Path) -> Vec<PluginMeta> {
    let stats = load_stats(dir);
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for entry in rd.flatten() {
            let p = entry.path();
            if p.extension().and_then(|e| e.to_str()) != Some("rhai") {
                continue;
            }
            let name = p
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            // 仅列出符合 `gen_<16 hex>` 命名规范的插件，与 delete 的安全约束保持一致。
            if !regex_name_ok(&name) {
                continue;
            }
            let source = std::fs::read_to_string(&p).unwrap_or_default();
            let trigger = source
                .lines()
                .find_map(|l| l.trim().strip_prefix("// trigger:"))
                .unwrap_or("")
                .trim()
                .to_string();
            let st = stats.get(&name).cloned().unwrap_or_default();
            out.push(PluginMeta {
                name,
                trigger,
                source,
                mtime: mtime_secs(&p),
                size: entry.metadata().map(|m| m.len()).unwrap_or(0),
                hits: st.hits,
                last_hit: st.last_hit,
            });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// 删除一个插件文件（名称须为 `gen_<16 hex>`，防目录穿越）；返回是否确有删除。
pub fn delete_plugin(name: &str, dir: &Path) -> bool {
    if !regex_name_ok(name) {
        return false;
    }
    let path = dir.join(format!("{name}.rhai"));
    if path.exists() && std::fs::remove_file(&path).is_ok() {
        invalidate_cache(dir);
        true
    } else {
        false
    }
}

fn regex_name_ok(name: &str) -> bool {
    let rest = name.strip_prefix("gen_").unwrap_or(name);
    !name.is_empty()
        && name.starts_with("gen_")
        && rest.chars().all(|c| c.is_ascii_hexdigit())
        && rest.len() == 16
}

/// 持久化一个已校验的检测器；同名源码幂等。返回插件名或拒绝原因。
pub fn save_plugin(source: &str, dir: &Path, trigger: &str) -> Result<String, HarnessError> {
    // 持久化前先编译校验，绝不把无法运行的代码写盘。
    if engine::build_engine().compile(source).is_err() {
        return Err(HarnessError::other("refusing to persist invalid plugin"));
    }
    let _ = std::fs::create_dir_all(dir);
    let name = plugin_name(source);
    let path = dir.join(format!("{name}.rhai"));
    if path.exists() {
        return Ok(name);
    }
    let one_line: String = trigger.split_whitespace().collect::<Vec<_>>().join(" ");
    let one_line: String = one_line.chars().take(80).collect();
    let body = format!("// trigger: {one_line}\n{source}");
    crate::agent::write_private(&path.to_string_lossy(), &body)
        .map_err(|e| HarnessError::other(format!("写入插件失败: {e}")))?;
    invalidate_cache(dir);
    Ok(name)
}
