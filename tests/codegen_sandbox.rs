//! 子进程沙箱集成测试：直接驱动隐藏子命令 `_codegen_run`，验证隔离执行与超时 kill。
//!
//! `CARGO_BIN_EXE_resolve-tui` 在集成测试里指向当前编译出的二进制，等价于生产路径。
#![cfg(feature = "codegen")]

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// 子进程输出：新协议 `{"i":<下标>,"a":<答案>}` 解析结果；无输出时两者为默认值。
struct ChildOut {
    index: Option<usize>,
    answer: String,
}

fn run_child(query: &str, sources: &[String]) -> ChildOut {
    let bin = env!("CARGO_BIN_EXE_resolve-tui");
    let req = serde_json::json!({ "query": query, "sources": sources });
    let mut child = Command::new(bin)
        .arg("_codegen_run")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("应能启动子进程");
    // 看门狗：若沙箱自身超时逻辑失效，12s 后强杀子进程，避免测试套件永久挂起。
    #[cfg(unix)]
    {
        let pid = child.id();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(12));
            unsafe {
                libc::kill(pid as libc::pid_t, libc::SIGKILL);
            }
        });
    }
    {
        let mut stdin = child.stdin.take().unwrap();
        stdin
            .write_all(serde_json::to_string(&req).unwrap().as_bytes())
            .unwrap();
        // 关闭 stdin 让子进程读到 EOF。
    }
    let mut out = String::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut out)
        .unwrap();
    let _ = child.wait().unwrap();
    match serde_json::from_str::<serde_json::Value>(&out) {
        Ok(v) if v.is_object() => ChildOut {
            index: v.get("i").and_then(|x| x.as_u64()).map(|i| i as usize),
            answer: v
                .get("a")
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string(),
        },
        _ => ChildOut {
            index: None,
            answer: out,
        },
    }
}

#[test]
fn sandbox_runs_detector_and_isolates() {
    // 正常命中：答案 + 源码下标一并返回。
    let src =
        "fn detect(text) { if text.contains(\"ping\") { \"pong\" } else { \"\" } }".to_string();
    let out = run_child("ping me", &[src]);
    assert_eq!(out.answer, "pong");
    assert_eq!(out.index, Some(0));

    // 未命中返回空。
    let src2 =
        "fn detect(text) { if text.contains(\"xyz\") { \"yes\" } else { \"\" } }".to_string();
    let out2 = run_child("hello", &[src2]);
    assert_eq!(out2.answer, "");
    assert_eq!(out2.index, None);

    // 多源：命中第一个即返回，且下标指向命中的源。
    let a = "fn detect(text) { if text.contains(\"a\") { \"A\" } else { \"\" } }".to_string();
    let b = "fn detect(text) { if text.contains(\"b\") { \"B\" } else { \"\" } }".to_string();
    let out3 = run_child("bbb", &[a, b]);
    assert_eq!(out3.answer, "B");
    assert_eq!(out3.index, Some(1));
}

#[test]
fn sandbox_kills_hanging_detector() {
    // 死循环检测器：子进程必须被自身 2s 逐源超时 + 父进程 budget 约束在合理时间内结束，
    // 绝不能永久挂起（本测试自身 15s 超时即视为失败）。
    let hang = "fn detect(text) { while true { } \"\" }".to_string();
    let good =
        "fn detect(text) { if text.contains(\"ping\") { \"pong\" } else { \"\" } }".to_string();
    let start = Instant::now();
    // 死循环源在前，验证它不会拖垮后续源（子进程内逐源 2s 超时隔离）。
    let out = run_child("ping me", &[hang.clone(), good.clone()]);
    let elapsed = start.elapsed();
    assert_eq!(out.answer, "pong", "死循环源不应阻断后续源命中");
    assert_eq!(out.index, Some(1), "下标应指向死循环之后的可用源");
    assert!(
        elapsed < Duration::from_secs(15),
        "子进程应在 budget 内结束，实际耗时 {elapsed:?}"
    );

    // 纯死循环、无任何命中：整体应在 budget 内返回空（被 kill），不挂起。
    let start2 = Instant::now();
    let out2 = run_child("anything", &[hang]);
    let elapsed2 = start2.elapsed();
    assert_eq!(out2.answer, "");
    assert_eq!(out2.index, None);
    assert!(
        elapsed2 < Duration::from_secs(15),
        "纯死循环应在 budget 内被杀，实际耗时 {elapsed2:?}"
    );
}
