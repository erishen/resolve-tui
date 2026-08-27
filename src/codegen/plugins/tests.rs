//! `plugins` 模块的单元测试：持久化→加载→执行、统计回环、上限淘汰、粗筛。

use crate::Config;
use crate::codegen::sandbox;

use super::cache::{PREFILTER_KEEP, evict_excess, load_plugins, prefilter_plugins, record_hits};
use super::*;

const SAMPLE: &str = r#"fn detect(text) {
    if text.contains("ping") { "pong" } else { "" }
}"#;

#[test]
fn save_then_load_then_run() {
    let dir = std::env::temp_dir().join(format!("harness_cg_test_{}.rhai", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    // 清空旧插件
    for p in std::fs::read_dir(&dir).unwrap().flatten() {
        let _ = std::fs::remove_file(p.path());
    }
    let name = save_plugin(SAMPLE, &dir, "ping me").expect("应保存");
    assert_eq!(name, plugin_name(SAMPLE));
    // 幂等：再存一次仍是同一文件名
    assert_eq!(save_plugin(SAMPLE, &dir, "ping me").unwrap(), name);

    let plugins = load_plugins(&dir);
    assert_eq!(plugins.len(), 1, "应加载到一个插件");
    assert_eq!(
        sandbox::try_detect(&plugins[0].src, "ping", std::time::Duration::from_secs(2)).ok(),
        Some(Some("pong".to_string()))
    );

    // 删除后目录为空
    assert!(delete_plugin(&name, &dir));
    assert!(!delete_plugin(&name, &dir), "删过的应返回 false");
    assert!(list_plugins(&dir).is_empty());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn delete_rejects_bad_name() {
    let dir = std::env::temp_dir();
    assert!(!delete_plugin("../escape", &dir));
    assert!(!delete_plugin("gen_zzzz", &dir));
}

// 命中统计：record_hits 累计后 list_plugins 应能读出 hits / last_hit。
#[test]
fn stats_roundtrip_and_list_merge() {
    let dir = std::env::temp_dir().join(format!("harness_cg_stat_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    for p in std::fs::read_dir(&dir).unwrap().flatten() {
        let _ = std::fs::remove_file(p.path());
    }
    let name = save_plugin(SAMPLE, &dir, "ping").unwrap();
    assert_eq!(list_plugins(&dir)[0].hits, 0, "初始无命中");

    record_hits(&dir, std::slice::from_ref(&name));
    record_hits(&dir, std::slice::from_ref(&name));
    let meta = &list_plugins(&dir)[0];
    assert_eq!(meta.hits, 2);
    assert!(meta.last_hit > 0, "最后命中时间应被记录");

    let _ = std::fs::remove_dir_all(&dir);
}

// 上限淘汰：超过 max 时淘汰冷插件（无命中者优先），保留有命中的。
#[test]
fn cap_evicts_coldest_plugins() {
    let dir = std::env::temp_dir().join(format!("harness_cg_cap_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    for p in std::fs::read_dir(&dir).unwrap().flatten() {
        let _ = std::fs::remove_file(p.path());
    }
    // 4 个不同源码的插件（源码必须互异才能生成不同文件名）。
    let mut hot_name = String::new();
    for i in 0..4 {
        let src = format!(
            "fn detect(text) {{ if text.contains(\"k{i}\") {{ \"{i}\" }} else {{ \"\" }} }}"
        );
        let name = save_plugin(&src, &dir, &format!("trigger {i}")).unwrap();
        if i == 2 {
            record_hits(&dir, std::slice::from_ref(&name));
            hot_name = name;
        }
    }
    evict_excess(&dir, 2);
    let survivors = list_plugins(&dir);
    assert_eq!(survivors.len(), 2, "应淘汰到上限 2 个");
    assert!(
        survivors.iter().any(|m| m.name == hot_name),
        "有命中的插件不应被淘汰"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

// 粗筛：插件多于 KEEP 时只保留前若干个，且与 query 最相关的必被保留。
#[test]
fn prefilter_keeps_relevant_and_caps_count() {
    let mk = |marker: &str| LoadedPlugin {
        name: format!("gen_{marker}"),
        src: format!("// trigger: {marker}\nfn detect(t) {{ \"\" }}"),
    };
    let mut plugins: Vec<LoadedPlugin> = (0..40).map(|i| mk(&format!("无关词{i}"))).collect();
    plugins.push(mk("订单号"));

    let picked = prefilter_plugins("我的订单号是多少", &plugins);
    assert!(picked.len() <= PREFILTER_KEEP, "粗筛应限制送入子进程的数量");
    assert!(
        picked.iter().any(|p| p.src.contains("订单号")),
        "与 query 最相关的插件必须被保留"
    );
    // 少量插件时不启用粗筛，原样全量返回。
    let few: Vec<LoadedPlugin> = (0..3).map(|i| mk(&format!("x{i}"))).collect();
    assert_eq!(prefilter_plugins("q", &few).len(), 3);
}

// 需真实 LLM 的端到端生成：仅在显式开启时运行，避免测试网络依赖。
#[tokio::test]
async fn llm_codegen_solve_end_to_end() {
    if std::env::var("HARNESS_LLM_TEST").is_err() {
        return;
    }
    let cfg = Config::from_env();
    let dir = std::env::temp_dir().join(format!("harness_cg_llm_{}", std::process::id()));
    for p in std::fs::read_dir(&dir).unwrap().flatten() {
        let _ = std::fs::remove_file(p.path());
    }
    let ans = try_codegen(
        "判断 2024 年是否为闰年并简要说明",
        &cfg,
        &cfg.model,
        Some(&dir),
    )
    .await
    .unwrap_or(None);
    // 不断言具体内容，只确认流程不崩；可能 None（模型拒绝/网络）。
    let _ = ans;
    let _ = std::fs::remove_dir_all(&dir);
}
