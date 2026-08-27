use std::path::{Path, PathBuf};

use crate::HarnessError;

/// 工具执行的隔离策略。
#[derive(Clone, Debug)]
pub struct SandboxPolicy {
    /// 是否启用沙箱。关闭后命令在本机直接执行。
    pub enabled: bool,
    /// 是否允许沙箱内访问网络。
    pub allow_network: bool,
    /// 允许写入的目录白名单。
    pub writable_roots: Vec<PathBuf>,
    /// 任务工作目录：工具相对路径以此为基准解析，shell 也在该目录执行。
    /// `None` 表示进程当前目录（无每任务隔离）。
    pub cwd: Option<PathBuf>,
}

impl SandboxPolicy {
    /// 默认可写根：当前工作目录 + 系统临时目录。
    pub fn default_roots() -> Vec<PathBuf> {
        vec![
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            std::env::temp_dir(),
        ]
    }

    /// 启用沙箱时，写文件类工具用该函数做应用层白名单校验。
    pub fn is_writable(&self, path: &Path) -> bool {
        if !self.enabled {
            return true;
        }
        let base = self.base();
        // 相对路径先基于任务工作区（或 cwd）归一化为绝对路径，否则匹配不上绝对
        // 白名单根；再做词法归一（解析 `.`/`..`），防止 `../` 逃逸绕过前缀匹配。
        let abs = normalize_path(&absolutize(path, &base));
        self.writable_roots
            .iter()
            .any(|root| abs.starts_with(normalize_path(&absolutize(root, &base))))
    }

    /// 工具相对路径解析为绝对路径：基于任务工作区（`cwd`），否则进程当前目录。
    pub fn resolve(&self, path: &Path) -> PathBuf {
        let base = self.base();
        absolutize(path, &base)
    }

    /// 读取路径基准：项目目录（启动 cwd）相对，而非工作区。
    /// 模型照「项目相对路径」读仓库文件是常态，落工作区会导致 ENOENT；
    /// 写才落工作区（见 [`resolve`]）。
    pub fn resolve_read(&self, path: &Path) -> PathBuf {
        let base = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        absolutize(path, &base)
    }

    /// 读文件范畴校验：项目目录（启动 cwd）+ 当前工作区 + 系统临时目录。
    /// 超出即拒绝，防止模型把盘外文件（`~/.ssh`、`/etc/passwd` 等）读进上下文再外发。
    pub fn is_readable(&self, path: &Path) -> bool {
        if !self.enabled {
            return true;
        }
        let abs = normalize_path(&self.resolve_read(path));
        roots_contained(&abs, &readable_roots(self))
    }

    /// 相对路径基准：`cwd` 优先，回退进程当前目录。
    fn base(&self) -> PathBuf {
        self.cwd
            .clone()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
    }
}

/// 读范围：白名单根（含工作区）+ 项目 cwd + 系统临时目录。
fn readable_roots(policy: &SandboxPolicy) -> Vec<PathBuf> {
    let mut roots = policy.writable_roots.clone();
    roots.push(std::env::current_dir().unwrap_or_default());
    roots.push(std::env::temp_dir());
    roots
}

/// `abs`（绝对路径）是否落在任一 root（均词法归一后 `starts_with`）内。
fn roots_contained(abs: &Path, roots: &[PathBuf]) -> bool {
    let base = std::env::current_dir().unwrap_or_default();
    roots.iter().any(|r| {
        let root = normalize_path(&absolutize(r, &base));
        abs.starts_with(&root)
    })
}

/// 把路径转为绝对路径：已是绝对则原样返回，否则按 `base` 拼接。
fn absolutize(path: &Path, base: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

/// 词法归一化：解析 `.` / `..`，不访问文件系统（不解析符号链接）。
fn normalize_path(p: &Path) -> PathBuf {
    use std::path::Component::*;

    let mut parts: Vec<std::ffi::OsString> = Vec::new();
    for comp in p.components() {
        match comp {
            CurDir => {}
            ParentDir => {
                parts.pop();
            }
            RootDir => parts.clear(),
            Prefix(_) | Normal(_) => parts.push(comp.as_os_str().to_os_string()),
        }
    }
    let mut out = PathBuf::new();
    if p.is_absolute() {
        out.push(std::path::MAIN_SEPARATOR.to_string());
    }
    for part in parts {
        out.push(part);
    }
    out
}

/// 确保沙箱根目录存在（项目下默认 `.resolve-tui-sandbox`，启动时创建）。
pub fn ensure_sandbox_root(root: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(root)
}

/// 注入 system prompt 的沙箱上下文：明确告知模型「写落工作区、读基于项目目录」，
/// 避免它瞎猜路径（此前会试写项目根下任意路径而频频被拒），也防止尝试读盘外文件。
pub fn prompt(policy: &SandboxPolicy) -> Option<String> {
    let cwd = policy.cwd.as_deref()?;
    let roots = policy
        .writable_roots
        .iter()
        .map(|r| r.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let read_scope = readable_roots(policy)
        .iter()
        .map(|r| r.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    Some(format!(
        "[沙箱] write_file / shell 的相对路径以工作区为准：{}。可写白名单：{roots}。\nread_file / list_dir 的相对路径以项目目录为准，可读范围：{read_scope}。\n写工作区之外会被拒绝，读取范围之外的文件会被拒绝。",
        cwd.display()
    ))
}

/// 旧任务工作区保留天数：启动时清理超过该时长的 `task-*`。
pub const WORKSPACE_RETENTION_SECS: u64 = 7 * 24 * 3600;

/// 清理超过 `max_age_secs` 的旧任务工作区（`task-*`），返回清理数量。
/// 启动时调用，避免 `.resolve-tui-sandbox` 长期只增不减。
pub fn prune_task_workspaces(root: &Path, max_age_secs: u64) -> usize {
    let now = std::time::SystemTime::now();
    let mut removed = 0;
    let Ok(rd) = std::fs::read_dir(root) else {
        return 0;
    };
    for entry in rd.flatten() {
        let p = entry.path();
        if !p.is_dir() {
            continue;
        }
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        if !name.starts_with("task-") {
            continue;
        }
        let age = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| now.duration_since(t).ok())
            .map(|d| d.as_secs())
            .unwrap_or(u64::MAX);
        if age > max_age_secs {
            let _ = std::fs::remove_dir_all(&p);
            removed += 1;
        }
    }
    removed
}

/// 为一次任务创建独立工作区 `<root>/task-<nanos>-<pid>/`，返回其绝对路径。
/// 各任务互不共享文件，产物互不覆盖。
pub fn new_task_workspace(root: &Path) -> Result<PathBuf, HarnessError> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let ws = root.join(format!("task-{nanos}-{}", std::process::id()));
    std::fs::create_dir_all(&ws)
        .map_err(|e| HarnessError::tool(format!("创建任务工作区失败: {e}")))?;
    Ok(ws)
}

/// 在策略约束下执行外部命令，返回 stdout（含非空 stderr 附注）。
pub async fn run_command(
    program: &str,
    args: &[&str],
    policy: &SandboxPolicy,
) -> Result<String, HarnessError> {
    let mut cmd = wrap_sandbox(program, args, policy)?;
    // 任务工作区：shell 命令在该目录内执行，相对路径落在工作区内。
    if let Some(cwd) = &policy.cwd {
        cmd.current_dir(cwd);
    }
    let out = cmd
        .output()
        .await
        .map_err(|e| HarnessError::tool(format!("执行 {program} 失败: {e}")))?;

    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    if !out.status.success() {
        return Err(HarnessError::tool(format!(
            "{program} 退出码 {:?}\nstdout: {}\nstderr: {}",
            out.status.code(),
            stdout.trim(),
            stderr.trim()
        )));
    }
    if stderr.trim().is_empty() {
        Ok(stdout)
    } else {
        Ok(format!("{stdout}\n[stderr] {}", stderr.trim()))
    }
}

/// 按平台把原始命令包进沙箱；沙箱程序缺失时降级为直接执行（打印告警）。
fn wrap_sandbox(
    program: &str,
    args: &[&str],
    policy: &SandboxPolicy,
) -> Result<tokio::process::Command, HarnessError> {
    if !policy.enabled {
        let mut cmd = tokio::process::Command::new(program);
        cmd.args(args);
        return Ok(cmd);
    }

    #[cfg(target_os = "macos")]
    {
        // macOS：seatbelt（sandbox-exec）。默认全拒，按需放行读/进程，写仅限白名单根。
        let profile_path = write_seatbelt_profile(policy)?;
        let mut cmd = tokio::process::Command::new("/usr/bin/sandbox-exec");
        cmd.arg("-f").arg(profile_path).arg(program).args(args);
        Ok(cmd)
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        const BWRAP: &str = "/usr/bin/bwrap";
        if !Path::new(BWRAP).exists() {
            eprintln!("[sandbox] 未找到 bwrap，降级为无沙箱执行");
            let mut cmd = tokio::process::Command::new(program);
            cmd.args(args);
            return Ok(cmd);
        }
        let mut cmd = tokio::process::Command::new(BWRAP);
        cmd.arg("--ro-bind").arg("/").arg("/");
        for root in &policy.writable_roots {
            cmd.arg("--bind").arg(root).arg(root);
        }
        cmd.args(["--dev", "/dev", "--proc", "/proc"]);
        if !policy.allow_network {
            cmd.arg("--unshare-net");
        }
        cmd.arg("--die-with-parent");
        cmd.arg(program).args(args);
        Ok(cmd)
    }

    #[cfg(not(unix))]
    {
        eprintln!("[sandbox] 当前平台不支持沙箱，降级为直接执行");
        let mut cmd = tokio::process::Command::new(program);
        cmd.args(args);
        Ok(cmd)
    }
}

/// 生成 seatbelt 配置到临时文件，返回其路径。
///
/// 每任务工作区不同（可写白名单随任务变化），故每次执行都重写配置——
/// 不做进程级缓存，否则后续任务会复用首个任务的过期白名单。
#[cfg(target_os = "macos")]
fn write_seatbelt_profile(policy: &SandboxPolicy) -> Result<PathBuf, HarnessError> {
    use std::fmt::Write as _;

    let mut sb = String::from("(version 1)\n(deny default)\n");
    sb.push_str("(allow process*)\n(allow mach-lookup)\n(allow file-read*)\n");
    // 写默认全拒，再按白名单根放行对应 subpath。
    // 说明：读保持 OS 全局可读——应用层 `is_readable` 已把 read_file/list_dir
    // 模型工具通道圈在工作区/项目内；shell 内读要收紧需枚举系统库路径，风险大于收益。
    sb.push_str("(deny file-write*)\n");
    for root in &policy.writable_roots {
        let root = root.canonicalize().unwrap_or_else(|_| root.clone());
        let _ = writeln!(sb, "(allow file-write* (subpath \"{}\"))", root.display());
    }
    if policy.allow_network {
        sb.push_str("(allow network*)\n(allow system-socket)\n");
    }

    let path = std::env::temp_dir().join(format!("resolve-tui-{}.sb", std::process::id()));
    std::fs::write(&path, sb)
        .map_err(|e| HarnessError::tool(format!("写入 seatbelt 配置失败: {e}")))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(cwd: &str) -> SandboxPolicy {
        SandboxPolicy {
            enabled: true,
            allow_network: false,
            writable_roots: vec![PathBuf::from(cwd)],
            cwd: None,
        }
    }

    /// 相对路径应归一化为 cwd 下的绝对路径后再比对白名单根。
    #[test]
    fn relative_path_is_writable_under_cwd() {
        let cwd = std::env::current_dir().unwrap();
        let p = policy(&cwd.to_string_lossy());
        assert!(
            p.is_writable(Path::new("src/lib.rs")),
            "相对路径应命中 cwd 白名单"
        );
        assert!(p.is_writable(Path::new("./src/lib.rs")), "显式 ./ 也应命中");
        assert!(
            p.is_writable(&cwd.join("src/lib.rs")),
            "绝对路径在 cwd 内应命中"
        );
    }

    /// `..` 逃逸必须被词法归一拦下：不能借 starts_with 前缀匹配绕过白名单。
    #[test]
    fn parent_escape_is_rejected() {
        let cwd = std::env::current_dir().unwrap();
        // cwd/../outside 归一化后落在 cwd 之外。
        let escape = cwd.join("..").join("outside.txt");
        let p = policy(&cwd.to_string_lossy());
        assert!(!p.is_writable(&escape), ".. 逃逸到白名单外应被拒绝");
        // 显式外部绝对路径更不必说。
        assert!(!p.is_writable(Path::new("/etc/passwd")));
    }

    /// 白名单根本身若是相对路径也应归一化后再比对。
    #[test]
    fn relative_root_is_normalized_too() {
        let p = policy(".");
        let cwd = std::env::current_dir().unwrap();
        assert!(p.is_writable(&cwd.join("anything.md")));
    }

    /// 任务工作区：相对路径以 cwd（工作区）为基准解析，且写只落在工作区内。
    #[test]
    fn task_workspace_scopes_relative_paths() {
        let ws = std::env::temp_dir().join(format!("resolve_tui_ws_{}", std::process::id()));
        let p = SandboxPolicy {
            enabled: true,
            allow_network: false,
            writable_roots: vec![ws.clone()],
            cwd: Some(ws.clone()),
        };
        // 相对路径解析到工作区内。
        let resolved = p.resolve(Path::new("src/lib.rs"));
        assert_eq!(resolved, ws.join("src/lib.rs"));
        assert!(p.is_writable(Path::new("src/lib.rs")));
        // 逃出工作区（含绝对路径与 ..）一律拒绝。
        assert!(!p.is_writable(Path::new("/etc/passwd")));
        assert!(!p.is_writable(&ws.join("..").join("out.md")));
        let _ = std::fs::remove_dir_all(&ws);
    }

    /// 每任务工作区目录互不相同且可独立创建。
    #[test]
    fn new_task_workspace_creates_distinct_dirs() {
        let root = std::env::temp_dir().join(format!("resolve_tui_wsroot_{}", std::process::id()));
        let a = new_task_workspace(&root).expect("创建 A 工作区失败");
        let b = new_task_workspace(&root).expect("创建 B 工作区失败");
        assert!(a.is_dir() && b.is_dir());
        assert_ne!(a, b, "各任务工作区必须独立");
        std::fs::write(a.join("x.txt"), "A").unwrap();
        std::fs::write(b.join("y.txt"), "B").unwrap();
        assert!(!a.join("y.txt").is_file(), "A 不应看到 B 的文件");
        assert!(!b.join("x.txt").is_file(), "B 不应看到 A 的文件");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// system prompt 沙箱提示：有工作区时给出路径与白名单；无工作区时为空。
    #[test]
    fn prompt_lists_writable_workspace_when_cwd_set() {
        let ws = std::env::temp_dir().join("resolve_tui_sb_hint");
        let no_ws = SandboxPolicy {
            enabled: true,
            allow_network: false,
            writable_roots: vec![ws.clone()],
            cwd: None,
        };
        assert!(prompt(&no_ws).is_none(), "无工作区不应注入提示");

        let with_ws = SandboxPolicy {
            cwd: Some(ws.clone()),
            ..no_ws.clone()
        };
        let hint = prompt(&with_ws).expect("有工作区应有提示");
        assert!(
            hint.contains(&ws.to_string_lossy().to_string()),
            "提示应包含可写工作区路径: {hint}"
        );
        assert!(hint.contains("可写白名单"), "提示应列出白名单");
        assert!(hint.contains("读取范围"), "提示应说明读取范围");
    }

    /// 读范畴：工作区 + 项目 cwd + 临时目录放行，盘外绝对路径拒绝。
    #[test]
    fn is_readable_scopes_to_workspace_project_and_temp() {
        let ws = std::env::temp_dir().join(format!("resolve_tui_readable_{}", std::process::id()));
        let p = SandboxPolicy {
            enabled: true,
            allow_network: false,
            writable_roots: vec![ws.clone()],
            cwd: Some(ws.clone()),
        };
        let project = std::env::current_dir().unwrap();
        assert_eq!(
            p.resolve_read(Path::new("resolve-tui/src/lib.rs")),
            project.join("resolve-tui/src/lib.rs"),
            "相对读应以项目目录为基准"
        );
        assert!(p.is_readable(Path::new("x.txt")), "项目内相对路径应可读");
        assert!(p.is_readable(&ws), "工作区可读");
        assert!(p.is_readable(&project), "项目目录可读");
        assert!(p.is_readable(&std::env::temp_dir()), "系统临时目录可读");
        assert!(
            !p.is_readable(Path::new("/etc/passwd")),
            "盘外系统文件应拒绝"
        );
        assert!(
            !p.is_readable(&std::env::home_dir().unwrap_or_default().join(".ssh/id_rsa")),
            "用户主目录敏感文件应拒绝"
        );
    }

    /// 工作区清理：只删过期的 task-*，跳过未过期与无关目录。
    #[cfg(unix)]
    #[test]
    fn prune_removes_only_expired_task_dirs() {
        let root = std::env::temp_dir().join(format!("resolve_tui_prune_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        let old = root.join("task-old");
        let fresh = root.join("task-fresh");
        let other = root.join("not-a-task");
        for d in [&old, &fresh, &other] {
            std::fs::create_dir(d).unwrap();
        }
        // 把 old 的 mtime 拨回 2000-01-01。
        std::process::Command::new("touch")
            .args(["-t", "200001010000"])
            .arg(&old)
            .status()
            .unwrap();

        let removed = prune_task_workspaces(&root, 1000);
        assert_eq!(removed, 1, "只应清理过期的 task-old，实际 {removed}");
        assert!(!old.exists(), "过期任务工作区应被删除");
        assert!(fresh.exists(), "未过期工作区应保留");
        assert!(other.exists(), "非 task-* 目录不应被清理");
        let _ = std::fs::remove_dir_all(&root);
    }
}
