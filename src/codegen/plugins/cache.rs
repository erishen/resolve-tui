//! 插件缓存与治理原语：按目录整体缓存、命中统计、零成本粗筛、上限淘汰。
//!
//! 这部分只做「内存/磁盘上的元数据与统计」，不触及模型调用；
//! 跨模块的缓存重载依赖 [`super::LoadedPlugin`] 与 [`super::registry`] 的持久化视图。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use serde::{Deserialize, Serialize};

use super::registry::list_plugins;
use super::{LoadedPlugin, mtime_secs, now_secs};

struct CacheEntry {
    stats: HashMap<String, (i64, u64)>,
    /// 以 `Arc` 持有：命中缓存时返回引用计数克隆，避免每次调用深拷贝全部 AST。
    plugins: Arc<Vec<LoadedPlugin>>,
}

static PLUGIN_CACHE: OnceLock<Mutex<HashMap<PathBuf, CacheEntry>>> = OnceLock::new();

pub(crate) fn invalidate_cache(dir: &Path) {
    if let Some(m) = PLUGIN_CACHE.get() {
        m.lock().unwrap_or_else(|e| e.into_inner()).remove(dir);
    }
}

/// 加载目录下所有 `*.rhai` 插件源码；目录内容（mtime/size）变化即整体重载，
/// 未变化则复用缓存。不做父进程编译校验——坏插件由子进程执行时以 Err 静默跳过。
/// 返回共享句柄（廉价克隆），内容不可变——失效只能整体重建。
pub(crate) fn load_plugins(dir: &Path) -> Arc<Vec<LoadedPlugin>> {
    let mut files: Vec<(String, i64, u64)> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for entry in rd.flatten() {
            let p = entry.path();
            if p.extension().and_then(|e| e.to_str()) == Some("rhai") {
                let name = p
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();
                let meta = match entry.metadata() {
                    Ok(m) => m,
                    Err(_) => continue,
                };
                files.push((name, mtime_secs(&p), meta.len()));
            }
        }
    }
    let stats: HashMap<String, (i64, u64)> = files
        .iter()
        .map(|(n, m, s)| (n.clone(), (*m, *s)))
        .collect();

    let mut cache = PLUGIN_CACHE
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let need_reload = !matches!(cache.get(dir), Some(c) if c.stats == stats);
    if need_reload {
        // 不在父进程做编译校验：坏插件交给子进程的 try_detect 以 Err 跳过，
        // 行为等价但省掉每次热重载的 N 次 rhai 编译（子进程反正要重编）。
        let mut plugins = Vec::new();
        for (name, _, _) in &files {
            let p = dir.join(format!("{name}.rhai"));
            if let Ok(src) = std::fs::read_to_string(&p) {
                plugins.push(LoadedPlugin {
                    name: name.clone(),
                    src: src.clone(),
                });
            }
        }
        cache.insert(
            dir.to_path_buf(),
            CacheEntry {
                stats,
                plugins: Arc::new(plugins),
            },
        );
    }
    cache
        .get(dir)
        .map(|c| Arc::clone(&c.plugins))
        .unwrap_or_default()
}

/// 单个插件的命中统计。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PluginStat {
    /// 累计命中次数。
    pub hits: u64,
    /// 最后命中的 Unix 时间戳（秒）；从未命中为 0。
    pub last_hit: i64,
}

/// 统计持久化格式：与插件同目录的 `plugins.json`；文件缺失/损坏视为空——统计绝不致命。
#[derive(Default, Serialize, Deserialize)]
struct PluginStatsFile {
    plugins: HashMap<String, PluginStat>,
}

fn stats_path(dir: &Path) -> PathBuf {
    dir.join("plugins.json")
}

pub(crate) fn load_stats(dir: &Path) -> HashMap<String, PluginStat> {
    std::fs::read_to_string(stats_path(dir))
        .ok()
        .and_then(|s| serde_json::from_str::<PluginStatsFile>(&s).ok())
        .map(|f| f.plugins)
        .unwrap_or_default()
}

/// 批量记录命中：hits+1、last_hit=now。写失败静默（只影响展示，不影响功能）。
pub(crate) fn record_hits(dir: &Path, names: &[String]) {
    let mut stats = load_stats(dir);
    let now = now_secs();
    for n in names {
        let st = stats.entry(n.clone()).or_default();
        st.hits += 1;
        st.last_hit = now;
    }
    let payload = PluginStatsFile { plugins: stats };
    if let Ok(json) = serde_json::to_string(&payload)
        && std::fs::create_dir_all(dir).is_ok()
    {
        let _ = crate::agent::write_private(&stats_path(dir).to_string_lossy(), &json);
    }
}

/// 粗筛保留数量：插件多于该值时，只把「与 query 字符集重合度最高」的前若干个
/// 送进子进程执行。宁多带、不漏带——漏带的代价是白白重新生成一个等价插件。
pub(crate) const PREFILTER_KEEP: usize = 32;

/// 插件库上限：超过后淘汰最冷的 gen_* 插件（命中少且久未命中），防止无限膨胀。
const MAX_PLUGINS: usize = 200;

/// 零成本预筛：按 query 与插件源码（含 trigger 注释）的字符集重合度打分，
/// 稳定排序后保留前 [`PREFILTER_KEEP`] 个。同分保持原序，保证行为可预期。
pub(crate) fn prefilter_plugins<'a>(
    query: &str,
    plugins: &'a [LoadedPlugin],
) -> Vec<&'a LoadedPlugin> {
    if plugins.len() <= PREFILTER_KEEP {
        return plugins.iter().collect();
    }
    let qset: HashSet<char> = query.chars().filter(|c| c.is_alphanumeric()).collect();
    let mut scored: Vec<(usize, &LoadedPlugin)> = plugins
        .iter()
        .map(|p| {
            let set: HashSet<char> = p.src.chars().filter(|c| c.is_alphanumeric()).collect();
            (qset.intersection(&set).count(), p)
        })
        .collect();
    scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
    scored.truncate(PREFILTER_KEEP);
    scored.into_iter().map(|(_, p)| p).collect()
}

/// 落盘新插件后调用：超过 [`MAX_PLUGINS`] 时按冷度淘汰
/// （命中次数少者优先，其次久未命中的）。
pub(crate) fn enforce_plugin_cap(dir: &Path) {
    evict_excess(dir, MAX_PLUGINS);
}

pub(crate) fn evict_excess(dir: &Path, max: usize) {
    let mut metas = list_plugins(dir);
    if metas.len() <= max {
        return;
    }
    metas.sort_by_key(|m| (m.hits, m.last_hit));
    let excess = metas.len() - max;
    for m in &metas[..excess] {
        let _ = std::fs::remove_file(dir.join(format!("{}.rhai", m.name)));
    }
    invalidate_cache(dir);
}
