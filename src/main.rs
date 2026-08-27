#[cfg(feature = "tui")]
use resolve_tui::run_tui;
use resolve_tui::{Config, run, sessions};
use std::sync::Arc;

#[tokio::main]
async fn main() {
    // panic 时恢复终端（离开备用屏 + 关 raw mode），否则崩溃后整个终端
    // 会停留在残破状态，后续任何程序的输出都会错位。
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        #[cfg(feature = "tui")]
        {
            crossterm::terminal::disable_raw_mode().ok();
            crossterm::execute!(
                std::io::stdout(),
                crossterm::terminal::LeaveAlternateScreen,
                crossterm::event::DisableBracketedPaste
            )
            .ok();
        }
        #[cfg(not(feature = "tui"))]
        let _ = info;
        default_hook(info);
    }));

    // 进入 TUI 前的诊断先收集到 notes：TUI 模式下作为系统消息呈现（避免直接打 stderr
    // 在备用屏外闪现），CLI 模式则在下方照旧打到 stderr。
    let mut startup_notes: Vec<String> = Vec::new();
    // 优先加载 crate 本地 .env（无论 cargo 从哪个目录启动），再回退到当前目录 .env。
    load_env(&mut startup_notes);
    dotenvy::dotenv().ok();

    let config = Config::load();
    // 配置合理性校验：仅告警不阻断，避免缺失可选项时直接退出。
    if let Err(e) = config.validate() {
        startup_notes.push(format!(
            "[main] 配置告警：{e}（将尝试继续使用，但可能无法正常调用模型）"
        ));
    }
    let args: Vec<String> = std::env::args().collect();

    // 隐藏子命令：在隔离子进程里执行 codegen 检测器（父进程通过超时 kill 兜底）。
    // 必须最先拦截，避免触发下面的 env 加载/日志/配置流程。
    #[cfg(feature = "codegen")]
    if args.get(1).map(|s| s.as_str()) == Some("_codegen_run") {
        run_codegen_child();
        return;
    }

    let tui_flag = args.iter().any(|a| a == "--tui" || a == "-t");

    // `codegen` 子命令：管理已缓存的 codegen 检测器插件（list / delete / clear）。
    // 仅当首个位置参数为 `codegen` 时进入，避免与正常任务文本混淆。
    #[cfg(feature = "codegen")]
    if args.get(1).map(|s| s.as_str()) == Some("codegen") {
        let sub: Vec<&String> = args[2..].iter().collect();
        codegen_manage(&sub);
        return;
    }
    // 未启用 codegen feature 时给出明确提示而不是当作任务文本。
    #[cfg(not(feature = "codegen"))]
    if matches!(
        args.get(1).map(|s| s.as_str()),
        Some("_codegen_run") | Some("codegen")
    ) {
        eprintln!("本二进制未启用 codegen feature（请用 --features codegen 重新编译）");
        std::process::exit(2);
    }

    // 解析 `--resume <key>` / `--resume=<key>`：key 可以是 序号 / 会话名 / 显式路径。
    let mut resume: Option<String> = None;
    let mut multi_agent_flag = false;
    let mut i = 1;
    while i < args.len() {
        let a = &args[i];
        if a == "--resume" && i + 1 < args.len() {
            resume = Some(args[i + 1].clone());
            i += 2;
        } else if let Some(rest) = a.strip_prefix("--resume=") {
            if !rest.is_empty() {
                resume = Some(rest.to_string());
            }
            i += 1;
        } else if a == "--multi-agent" {
            multi_agent_flag = true;
            i += 1;
        } else {
            i += 1;
        }
    }

    // 多 Agent 开关：覆盖配置，开启三角色（Planner/Specialist/Evaluator）编排。
    let config = if multi_agent_flag {
        Arc::new(Config {
            multi_agent: true,
            ..(*config).clone()
        })
    } else {
        config
    };

    // git stash 风格：`--resume list` 列出会话后退出。
    if resume.as_deref() == Some("list") {
        print_session_list();
        return;
    }

    // 指定 --resume 时强制进入 TUI（会话续接是交互式功能）。
    let tui = tui_flag || resume.is_some();

    if tui {
        #[cfg(feature = "tui")]
        {
            if let Err(e) = run_tui(config, resume, startup_notes).await {
                eprintln!("[main] tui error: {e}");
                std::process::exit(1);
            }
            return;
        }
        #[cfg(not(feature = "tui"))]
        {
            let _ = resume;
            eprintln!("本二进制未启用 tui feature（请用 --features tui 重新编译）");
            std::process::exit(2);
        }
    }

    // CLI 模式：把启动诊断照旧打到 stderr（TUI 模式已在聊天区呈现）。
    for note in &startup_notes {
        eprintln!("{note}");
    }

    let task = parse_task().unwrap_or_else(|| {
        eprintln!("用法: resolve-tui \"<任务>\"  (或 --tui 进入交互界面；--resume list 查看会话)");
        std::process::exit(2);
    });

    println!("[main] 任务: {task}");
    println!("[main] 模型: {} | base: {}", config.model, config.api_base);

    match run(&task, &config).await {
        Ok(_) => println!("\n[main] 任务完成。"),
        Err(e) => {
            eprintln!("[main] 失败: {e}");
            std::process::exit(1);
        }
    }
}

/// `codegen` 插件管理子命令：list / delete <name> / clear。
/// 目录与运行时一致：config（toml/env）指定了 `codegen_plugin_dir` 就用它，
/// 否则用系统默认位置——避免「管理的」和「生效的」不是同一份插件。
#[cfg(feature = "codegen")]
fn codegen_manage(sub: &[&String]) {
    use resolve_tui::codegen::{codegen_plugin_dir, delete_plugin, list_plugins};

    let cfg = Config::load();
    let dir = cfg
        .codegen_plugin_dir
        .clone()
        .unwrap_or_else(codegen_plugin_dir);
    if sub.is_empty() || sub[0] == "list" {
        let plugins = list_plugins(&dir);
        if plugins.is_empty() {
            println!("没有已缓存的 codegen 插件（目录 {}）", dir.display());
            return;
        }
        println!("已缓存的 codegen 插件（目录 {}）：", dir.display());
        for (i, p) in plugins.iter().enumerate() {
            let trigger = if p.trigger.is_empty() {
                "（无触发描述）".to_string()
            } else {
                p.trigger.clone()
            };
            println!(
                "[{}] {:<22} 触发: {}  大小: {}B  命中: {}  最近: {}",
                i,
                p.name,
                trigger,
                p.size,
                p.hits,
                format_last_hit(p.last_hit)
            );
        }
        return;
    }

    match sub[0].as_str() {
        "delete" => {
            let name = sub.get(1).map(|s| s.as_str()).unwrap_or("");
            if name.is_empty() {
                eprintln!("用法: resolve-tui codegen delete <文件名（含 .rhai 或省略）>");
                std::process::exit(2);
            }
            match delete_plugin(name, &dir) {
                true => println!("已删除插件: {name}"),
                false => {
                    eprintln!("未找到插件: {name}（用 `codegen list` 查看）");
                    std::process::exit(1);
                }
            }
        }
        "clear" => {
            let plugins = list_plugins(&dir);
            let mut n = 0;
            for p in &plugins {
                if delete_plugin(&p.name, &dir) {
                    n += 1;
                }
            }
            println!("已清空 {n} 个 codegen 插件");
        }
        other => {
            eprintln!("未知子命令: {other}（支持 list / delete <name> / clear）");
            std::process::exit(2);
        }
    }
}

/// 子进程沙箱执行入口：转交 `codegen::run_codegen_child`（实现见库，含协议解析与执行）。
#[cfg(feature = "codegen")]
fn run_codegen_child() {
    resolve_tui::codegen::run_codegen_child();
}

/// 把最后命中的 Unix 时间戳渲染成本地时间（从未命中显示 `-`）。
#[cfg(feature = "codegen")]
fn format_last_hit(ts: i64) -> String {
    if ts <= 0 {
        return "-".to_string();
    }
    chrono::DateTime::from_timestamp(ts, 0)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_else(|| "-".to_string())
}

/// git stash 风格的会话列表输出。
fn print_session_list() {
    let dir = sessions::sessions_dir();
    let items = sessions::list(&dir);
    if items.is_empty() {
        println!("没有已保存的会话（目录 {}）", dir.display());
        println!("提示：在 TUI 里用 /create <名称> 或 /save 归档对话。");
        return;
    }
    println!("已保存的会话（目录 {}）：", dir.display());
    for s in &items {
        let preview = if s.preview.is_empty() {
            "（空）".to_string()
        } else {
            s.preview.clone()
        };
        println!("[{}] {:<24} {}  {}", s.index, s.name, s.modified, preview);
    }
    println!("载入其中一个：resolve-tui --resume <索引|名称>");
}

/// 任务取自首参数；若没有参数则从 stdin 整段读取。
fn parse_task() -> Option<String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if !args.is_empty() {
        return Some(args.join(" "));
    }
    use std::io::Read;
    let mut buf = String::new();
    if std::io::stdin().read_to_string(&mut buf).is_ok() && !buf.trim().is_empty() {
        return Some(buf.trim().to_string());
    }
    None
}

/// 加载 crate 目录下的 `.env`（`CARGO_MANIFEST_DIR` 在编译期确定包源码位置）。
/// 这样即使从 workspace 根 `cargo run -p resolve-tui`，也能读到本服务的配置。
/// 诊断信息（如 .env 权限过开）收集到 `notes`，由调用方按运行模式呈现，
/// 避免在 TUI 备用屏外直接打 stderr 造成闪屏。
fn load_env(notes: &mut Vec<String>) {
    let env_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".env");
    if env_path.exists() {
        // .env 内含 API key：权限过开（组/其它可读）时提示收紧，防止同机其它用户读取。
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            if let Ok(mode) = std::fs::metadata(&env_path).map(|m| m.permissions().mode())
                && mode & 0o077 != 0
            {
                notes.push(format!(
                    "[main] 提示：.env 权限过于开放（{mode:o}，内含 API key），建议执行 chmod 600 {}",
                    env_path.display()
                ));
            }
        }
        let _ = dotenvy::from_path(&env_path);
    }
}
