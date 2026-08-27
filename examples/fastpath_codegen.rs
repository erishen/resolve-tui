//! fastpath 与 codegen 使用示例（**离线可跑，无需网络/模型**）。
//!
//! 运行：
//! ```sh
//! cargo run -p resolve-tui --example fastpath_codegen
//! ```
//!
//! - fastpath：纯代码可解的问题（算术/单位/日期/进制/统计）直接短路返回，零模型往返。
//! - codegen：把模型生成的受限 rhai 检测器在沙箱里执行；本示例用「模拟的模型回复」
//!   演示抽取 → 执行 → 持久化为插件的完整离线流程（生产环境这一步由 LLM 生成）。

// 未启用 codegen feature 时空实现，保证 --no-default-features 下也能编译。
#[cfg(not(feature = "codegen"))]
fn main() {}

#[cfg(feature = "codegen")]
use resolve_tui::{codegen, fastpath};
#[cfg(feature = "codegen")]
use std::time::Duration;

#[cfg(feature = "codegen")]
fn main() {
    println!("===== fastpath（确定性快路径，零模型）=====");
    let fast_cases = [
        "计算 (12 + 3) * 4 等于多少",
        "1英里等于多少公里",
        "2024-01-01 加 100 天是哪天",
        "把 255 从十进制转成十六进制",
        "这组数 3 1 4 1 5 9 2 6 5 3 的平均值和最大值",
        "今天天气不错", // 故意放一个不命中的，展示 None 分支
    ];
    for q in fast_cases {
        match fastpath::try_fast_answer(q, None) {
            Some(a) => println!(
                "- {q}\n    [{}] {}  （过程：{}）",
                a.method, a.answer, a.detail
            ),
            None => println!("- {q}\n    （未命中 fastpath，会下沉到 codegen / 普通 agent 循环）"),
        }
    }

    println!("\n===== codegen（受限 rhai 检测器，沙箱执行）=====");
    // 模拟「模型返回」：一段被围栏包裹的 rhai 检测器。
    let model_reply = r##"好的，用这个检测器：
```rhai
fn detect(text) {
    let m = regex_capture(text, #"订单\s*号\s*是\s*(\w+)"#);
    if m == "" { "" } else { "订单号是: " + m }
}
```
"##;
    let src = codegen::extract_code(model_reply).expect("应抽出检测器源码");
    println!("抽取出的检测器源码：\n{src}");

    // 离线执行（进程内；生产路径用等价的子进程沙箱，超时即 kill，无泄漏）。
    let query = "我的订单号是 ORD-9981 请查下";
    match codegen::detect_sources(query, std::slice::from_ref(&src), Duration::from_secs(3)) {
        Some(ans) => println!("命中答案：{ans}"),
        None => println!("未命中（返回空串）"),
    }

    // 插件持久化 + 管理 API 演示。
    let dir = std::env::temp_dir().join("resolve_tui_example_plugins");
    let _ = std::fs::create_dir_all(&dir);
    for p in std::fs::read_dir(&dir).unwrap().flatten() {
        let _ = std::fs::remove_file(p.path());
    }
    let name = codegen::save_plugin(&src, &dir, "抽取订单号").expect("应保存插件");
    println!("\n已保存插件：{name}");
    for p in codegen::list_plugins(&dir) {
        println!("  列出：{}  触发描述：{}", p.name, p.trigger);
    }
    assert!(codegen::delete_plugin(&name, &dir), "删除应成功");
    println!("已删除插件：{name}");
    let _ = std::fs::remove_dir_all(&dir);

    println!(
        "\n（提示：生产环境为 answer-first 两段式——每轮先查缓存插件（codegen_cached_answer，零模型），"
    );
    println!(
        "主循环答完后再由 codegen_learn 事后生成检测器入库；生成代码在独立子进程沙箱执行，超时即杀。）"
    );
}
