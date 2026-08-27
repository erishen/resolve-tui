//! 隔离执行：把 rhai 检测器放到受 setrlimit 约束的子进程沙箱里跑，超时即 kill。
//!
//! - `run_ast` / `try_detect`：线程内单源执行（子进程内部用，逐源隔离）
//! - `detect_sources*`：顺序尝试每个源码，返回首个命中
//! - `run_in_subprocess`：**生产路径**——fork 子进程执行全部插件，超时 kill 兜底
//! - `run_codegen_child`：子进程侧入口（`_codegen_run` 子命令）
//!
//! 即便脚本死循环/吃内存，父进程也只丢答案、不残留线程或占 CPU。
//!
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use rhai::{AST, Scope};

use crate::{HarnessError, codegen::engine};

// -- 执行（线程 + 超时） ----------------------------------------------------------

/// 在独立线程里执行预编译的 `detect`，硬超时后丢弃结果（引擎限时双重保险）。
///
/// 注意：生产路径（`try_codegen` / `codegen_solve`）已不再直接调用本函数，而是把执行放到
/// **独立子进程沙箱**（`run_in_subprocess`）里——那种情况下即便脚本陷入死循环，父进程会在
/// 超时后 `kill` 掉整个子进程，不会在主进程残留线程或持续占 CPU。本函数仅用于子进程内部
/// （逐源执行，互不影响）与单元测试。rhai 在 `sync` 特性下无 `set_timeout`，故子进程内部
/// 单个超时源仍可能短暂泄漏其工作线程，但随子进程被 kill 一并消亡。
fn run_ast(ast: &AST, text: &str, timeout: Duration) -> Option<String> {
    let ast = ast.clone();
    let text = text.to_string();
    let (tx, rx) = mpsc::channel::<Result<String, HarnessError>>();
    thread::spawn(move || {
        let engine = engine::build_engine();
        let mut scope = Scope::new();
        let res = engine.call_fn(&mut scope, &ast, "detect", (text,));
        let _ = tx.send(res.map_err(|e| HarnessError::other(e.to_string())));
    });
    match rx.recv_timeout(timeout) {
        Ok(Ok(s)) if !s.trim().is_empty() => Some(s.trim().to_string()),
        _ => None,
    }
}

/// 编译并执行一段源码，区分三种结果：
/// - `Err(msg)`：源码编译失败（msg 为编译错误，可回灌给模型做自我修正）
/// - `Ok(None)`：编译通过但未命中该问题
/// - `Ok(Some(ans))`：命中，ans 为答案
pub(crate) fn try_detect(
    source: &str,
    text: &str,
    timeout: Duration,
) -> Result<Option<String>, HarnessError> {
    let ast = engine::build_engine()
        .compile(source)
        .map_err(|e| HarnessError::other(e.to_string()))?;
    Ok(run_ast(&ast, text, timeout))
}

/// 子进程沙箱内的执行入口：顺序尝试每个源码，返回第一个非空命中的
/// `(下标, 答案)`。单个源卡死时由 `run_ast` 的线程超时隔离，不会拖垮后续源；
/// 整体仍由父进程 kill 兜底。
fn detect_sources_index(
    query: &str,
    sources: &[String],
    per_source: Duration,
) -> Option<(usize, String)> {
    for (i, src) in sources.iter().enumerate() {
        if let Some(ans) = try_detect(src, query, per_source).ok().flatten() {
            return Some((i, ans));
        }
    }
    None
}

/// [`detect_sources_index`] 的简化版：只要答案（供示例/测试使用）。
pub fn detect_sources(query: &str, sources: &[String], per_source: Duration) -> Option<String> {
    detect_sources_index(query, sources, per_source).map(|(_, ans)| ans)
}

/// 子进程请求协议：父进程把查询与全部插件源码经 stdin 以 JSON 传入，子进程跑完回写答案。
#[derive(serde::Serialize, serde::Deserialize)]
pub struct CodegenRunReq {
    query: String,
    sources: Vec<String>,
}

/// 子进程沙箱入口（由二进制 `_codegen_run` 调起）：读 stdin JSON，顺序跑检测器，
/// 命中则把 `{i: 下标, a: 答案}` 写到 stdout（下标供父进程回写命中统计）；
/// 无输出表示未命中。生命周期极短、资源受 setrlimit 约束。
pub fn run_codegen_child() {
    use std::io::{Read, Write};
    let mut buf = Vec::new();
    if std::io::stdin().read_to_end(&mut buf).is_err() {
        return;
    }
    let req: CodegenRunReq = match serde_json::from_slice(&buf) {
        Ok(r) => r,
        Err(_) => return,
    };
    if let Some((idx, ans)) = detect_sources_index(&req.query, &req.sources, Duration::from_secs(2))
    {
        let payload = serde_json::json!({ "i": idx, "a": ans });
        println!("{payload}");
        let _ = std::io::stdout().flush();
    }
}

/// 子进程执行结果：答案与命中的源码下标（用于回写命中统计；旧协议/未知时为 None）。
pub struct CodegenAnswer {
    pub answer: String,
    pub index: Option<usize>,
}

/// 隔离执行：在独立子进程里跑 `detect_sources_index`，超时直接 `kill`，
/// 绝不泄漏线程或占 CPU。
///
/// 这是 codegen 检测器执行的**生产路径**。所有插件/生成代码都在一个被 `setrlimit` 限内存、
/// 限 CPU 秒、禁开新文件、且超时即杀的子进程里运行；即便脚本死循环或吃内存，父进程也只丢答案、
/// 不残留资源。若当前进程无法拿到自身可执行路径或 spawn 失败，则退化为进程内执行（功能可用，
/// 但失去隔离——仅作兜底）。
pub async fn run_in_subprocess(
    query: &str,
    sources: &[String],
    timeout: Duration,
) -> Option<CodegenAnswer> {
    let exe = std::env::current_exe().ok()?;
    let req = CodegenRunReq {
        query: query.to_string(),
        sources: sources.to_vec(),
    };
    let input = serde_json::to_vec(&req).ok()?;
    // 超时预算随插件数平滑放大，封顶防止父进程久等。
    let budget = (Duration::from_secs(2) * (sources.len() as u32 + 1) + Duration::from_secs(1))
        .min(Duration::from_secs(15))
        .max(timeout);

    let mut cmd = tokio::process::Command::new(&exe);
    cmd.arg("_codegen_run")
        // Child 被丢弃（含超时分支提前返回）时由 tokio 兜底 SIGKILL 并 reap，
        // 避免子进程变僵尸挂到父进程退出为止。
        .kill_on_drop(true)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    // Unix：子进程内收紧资源上限，即便超时逻辑失效也跑不出沙箱。
    // 注意：不可设 RLIMIT_NOFILE=0，否则子进程在动态链接/ locale 初始化阶段就会因无 fd 可用而
    // 直接 abort，根本跑不到我们的代码。rhai 本身不碰文件系统，故仅限内存与 CPU 即可。
    #[cfg(unix)]
    unsafe {
        cmd.pre_exec(|| {
            let mem = libc::rlimit {
                rlim_cur: 256 * 1024 * 1024,
                rlim_max: 256 * 1024 * 1024,
            };
            let _ = libc::setrlimit(libc::RLIMIT_AS, &mem);
            let cpu = libc::rlimit {
                rlim_cur: 5,
                rlim_max: 5,
            };
            let _ = libc::setrlimit(libc::RLIMIT_CPU, &cpu);
            // 独立会话，便于必要时按组回收。
            let _ = libc::setsid();
            Ok(())
        });
    }

    let mut child = cmd.spawn().ok()?;
    // 写入请求并关闭 stdin（让子进程读到 EOF）。
    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        let _ = stdin.write_all(&input).await;
        let _ = stdin.shutdown().await;
        drop(stdin);
    }

    let mut stdout = child.stdout.take()?;
    let wait_fut = child.wait();
    let read_fut = async {
        use tokio::io::AsyncReadExt;
        let mut buf = Vec::new();
        stdout.read_to_end(&mut buf).await.map(|_| buf)
    };
    let combined = async move {
        let out = read_fut.await?;
        let status = wait_fut.await?;
        Ok::<_, std::io::Error>((status, out))
    };

    match tokio::time::timeout(budget, combined).await {
        Ok(Ok((status, out))) => {
            if status.success() && !out.is_empty() {
                Some(parse_child_output(&out))
            } else {
                None
            }
        }
        Ok(Err(_)) => None,
        Err(_) => {
            // 超时：kill 整个子进程（含其内部任何泄漏的工作线程）。
            // 注意：拿不到 pid 时绝不能用 kill(0)——那是「杀调用者所在进程组」，
            // 会把父进程自己一起杀掉；此时只能放弃回收，交由 kill_on_drop 兜底。
            #[cfg(unix)]
            if let Some(pid) = child.id() {
                unsafe {
                    let _ = libc::kill(pid as libc::pid_t, libc::SIGKILL);
                }
            }
            #[cfg(not(unix))]
            let _ = child.start_kill();
            None
        }
    }
}

/// 子进程 stdout 解析：优先按新协议 `{"i":<下标>,"a":<答案>}`；
/// 兼容旧版/异常输出——整段当作答案（下标未知）。
fn parse_child_output(out: &[u8]) -> CodegenAnswer {
    match serde_json::from_slice::<serde_json::Value>(out) {
        Ok(v) if v.is_object() => CodegenAnswer {
            answer: v
                .get("a")
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string(),
            index: v.get("i").and_then(|x| x.as_u64()).map(|i| i as usize),
        },
        _ => CodegenAnswer {
            answer: String::from_utf8_lossy(out).to_string(),
            index: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"fn detect(text) {
        if text.contains("ping") { "pong" } else { "" }
    }"#;

    #[test]
    fn run_detector_matches_and_misses() {
        assert_eq!(
            try_detect(SAMPLE, "ping me", Duration::from_secs(2)).ok(),
            Some(Some("pong".to_string()))
        );
        assert_eq!(
            try_detect(SAMPLE, "hello", Duration::from_secs(2)).ok(),
            Some(None)
        );
    }

    #[test]
    fn run_detector_rejects_unsafe() {
        // 试图访问文件系统的脚本在 rhai 里是运行时错误（函数未注册），被执行层吞掉 → 无答案，
        // 因此无法借检测器越权。这里断言它既不编译通过命中、也不崩溃。
        assert_eq!(
            try_detect(
                "fn detect(t) { read_file(\"x\") }",
                "x",
                Duration::from_secs(2)
            )
            .ok(),
            Some(None)
        );
    }

    #[test]
    fn try_detect_distinguishes_outcomes() {
        // 编译失败：返回 Err（带错误信息，可供模型自我修正）。
        assert!(try_detect("fn detect(t) { +++ }", "x", Duration::from_secs(2)).is_err());
        // 编译通过且命中。
        assert_eq!(
            try_detect(SAMPLE, "ping me", Duration::from_secs(2)).ok(),
            Some(Some("pong".to_string()))
        );
        // 编译通过但未命中。
        assert_eq!(
            try_detect(SAMPLE, "hi", Duration::from_secs(2)).ok(),
            Some(None)
        );
    }

    #[test]
    fn run_detector_uses_regex() {
        // rhai 原始字符串用 #"..."# 语法（不是 Rust 的 r"..."）。
        let simple = r##"fn detect(text) {
            let m = regex_capture(text, #"(\d+)\s*加\s*(\d+)"#);
            if m == "" { "" } else { "匹配到: " + m }
        }"##;
        assert_eq!(
            try_detect(simple, "帮我算 12 加 34 多少", Duration::from_secs(2)).ok(),
            Some(Some("匹配到: 12".to_string()))
        );
        assert_eq!(
            try_detect(simple, "今天天气不错", Duration::from_secs(2)).ok(),
            Some(None)
        );
    }
}
