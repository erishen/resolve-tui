//! Skills：轻量「提示词包」。每个 `<skill>/SKILL.md` 是一组领域指令，遵循
//! `resolve-skills` 仓库的 [`SKILL_SPEC`](https://github.com/...) 契约（对齐
//! Agent Skills 开放标准）。
//!
//! 激活策略为「自适应」：技能若声明了 `triggers`，则用户输入命中关键词才把正文
//! 注入当轮 system prompt（省 token）；若未声明 `triggers`（模型自选技能），
//! 正文常驻、由模型决定何时采用。
//!
//! 不需要协议支持，纯文本即可扩展 agent 能力。

use std::path::{Path, PathBuf};

/// 单个技能：front matter 元数据 + 指令正文 + 可选资源目录。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Skill {
    /// 技能名（契约要求 == 所在目录名）。
    pub name: String,
    pub description: String,
    /// 触发词：与用户输入做大小写不敏感的子串匹配。
    pub triggers: Vec<String>,
    pub body: String,
    /// 可选可执行脚本目录（`<skill>/scripts`）。
    pub scripts_dir: Option<PathBuf>,
    /// 可选参考资料目录（`<skill>/references`）。
    pub references_dir: Option<PathBuf>,
    /// 可选资产目录（`<skill>/assets`）。
    pub assets_dir: Option<PathBuf>,
}

/// 技能目录查找顺序：
/// 1. `$HARNESS_SKILLS_DIR`（显式指定，可指向 `resolve-skills` 仓库的 `skills/`）
/// 2. 当前工作目录下 `.resolve-tui-skills/`
/// 3. crate 内捆绑的 `resolve-skills/skills/`（git submodule）
/// 4. crate 安装目录下 `.resolve-tui-skills/`（兜底：从任意目录启动都能找到随包技能）
pub fn skills_dir() -> PathBuf {
    if let Ok(d) = std::env::var("HARNESS_SKILLS_DIR")
        && !d.trim().is_empty()
    {
        return PathBuf::from(d.trim());
    }
    let cwd = PathBuf::from(".resolve-tui-skills");
    if cwd.is_dir() {
        return cwd;
    }
    let bundled = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("resolve-skills")
        .join("skills");
    if bundled.is_dir() {
        return bundled;
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".resolve-tui-skills")
}

/// 角色（soul）目录查找顺序，与 [`skills_dir`] 平行：
/// 1. `$HARNESS_SOULS_DIR`（显式指定，可指向 `resolve-skills` 仓库的 `souls/`）
/// 2. crate 内捆绑的 `resolve-skills/souls/`（git submodule）
/// 3. crate 安装目录下的 `.resolve-tui-souls/`（兜底）
pub fn souls_dir() -> PathBuf {
    if let Ok(d) = std::env::var("HARNESS_SOULS_DIR")
        && !d.trim().is_empty()
    {
        return PathBuf::from(d.trim());
    }
    let bundled = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("resolve-skills")
        .join("souls");
    if bundled.is_dir() {
        return bundled;
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".resolve-tui-souls")
}

/// 按角色名取 soul 正文（即该角色的 system 指令）。
pub fn role_soul<'a>(souls: &'a [Skill], name: &str) -> Option<&'a str> {
    souls
        .iter()
        .find(|s| s.name == name)
        .map(|s| s.body.as_str())
}

/// 递归加载目录下全部 `<skill>/SKILL.md`（目录不存在 → 空列表；单个坏文件不影响其它文件）。
/// 第二个返回值是加载告警，调用方决定如何呈现（TUI 内作为系统消息、CLI 打到 stderr）。
pub fn load_skills(dir: &Path) -> (Vec<Skill>, Vec<String>) {
    load_docs(dir, "SKILL.md")
}

/// 加载角色（soul）定义：与 [`load_skills`] 同结构，只是文件名约定为 `SOUL.md`。
pub fn load_souls(dir: &Path) -> (Vec<Skill>, Vec<String>) {
    load_docs(dir, "SOUL.md")
}

/// 通用递归加载器：按给定的文档文件名（SKILL.md / SOUL.md）收集 `<name>/<file>`。
/// 返回已解析的技能/角色列表，以及加载过程中跳过的坏文件警告——
/// 警告不再直接打到 stderr（会污染 TUI 备用屏），改由调用方按运行模式呈现。
fn load_docs(dir: &Path, filename: &str) -> (Vec<Skill>, Vec<String>) {
    let mut out = Vec::new();
    let mut warnings = Vec::new();
    collect_docs(dir, filename, 0, &mut out, &mut warnings);
    out.sort_by(|a, b| a.name.cmp(&b.name));
    (out, warnings)
}

fn collect_docs(
    dir: &Path,
    filename: &str,
    depth: usize,
    out: &mut Vec<Skill>,
    warnings: &mut Vec<String>,
) {
    if depth > 6 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_docs(&path, filename, depth + 1, out, warnings);
        } else if path.file_name().is_some_and(|n| n == filename) {
            let skill_dir = path.parent().unwrap_or(dir).to_path_buf();
            let stem = skill_dir
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            match std::fs::read_to_string(&path) {
                Ok(content) => match parse_skill(&content, &stem) {
                    Some(mut skill) => {
                        skill.scripts_dir = Some(skill_dir.join("scripts")).filter(|p| p.is_dir());
                        skill.references_dir =
                            Some(skill_dir.join("references")).filter(|p| p.is_dir());
                        skill.assets_dir = Some(skill_dir.join("assets")).filter(|p| p.is_dir());
                        out.push(skill);
                    }
                    None => warnings.push(format!(
                        "[skills] 跳过无法解析的技能文件: {}",
                        path.display()
                    )),
                },
                Err(e) => warnings.push(format!("[skills] 读取 {} 失败: {e}", path.display())),
            }
        }
    }
}

/// 解析单个技能文件：
///
/// ```text
/// ---
/// name: rust-review
/// description: Rust 代码评审
/// triggers: review, 代码审查
/// ---
/// 正文指令……
/// ```
///
/// 未识别的 front matter 键一律忽略（保证 Claude Code / Codex 产出的技能可无改动加载）。
/// 无 front matter 时整个内容视为正文，name 取目录名 stem。
pub fn parse_skill(content: &str, fallback_name: &str) -> Option<Skill> {
    let trimmed = content.trim_start_matches('\u{feff}');
    let mut lines = trimmed.lines();
    if lines.next()?.trim() != "---" {
        // 无 front matter：整体作为正文。
        let body = trimmed.trim().to_string();
        if body.is_empty() {
            return None;
        }
        return Some(Skill {
            name: fallback_name.to_string(),
            description: String::new(),
            triggers: Vec::new(),
            body,
            ..Default::default()
        });
    }

    let mut name = String::new();
    let mut description = String::new();
    let mut triggers = Vec::new();
    let mut meta_ended = false;
    let rest: Vec<&str> = trimmed.lines().collect();
    let mut body_start = rest.len();
    for (i, line) in rest.iter().enumerate().skip(1) {
        if line.trim() == "---" {
            body_start = i + 1;
            meta_ended = true;
            break;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "name" => name = value.to_string(),
            "description" => description = value.to_string(),
            "triggers" => {
                triggers = value
                    .split([',', '，', ';', '；'])
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect();
            }
            // 未知键（含 Agent Skills 的 when_to_use / allowed-tools / agents 等）一律忽略。
            _ => {}
        }
    }
    if !meta_ended {
        return None;
    }
    let body = rest[body_start..].join("\n").trim().to_string();
    if body.is_empty() {
        return None;
    }
    Some(Skill {
        name: if name.is_empty() {
            fallback_name.to_string()
        } else {
            name
        },
        description,
        triggers,
        body,
        ..Default::default()
    })
}

/// 触发判定：任一触发词（大小写不敏感）是用户输入的子串即命中；
/// 未配置触发词的技能永不通过关键词命中（仍会作为模型自选技能常驻，见 [`active_bodies`]）。
pub fn matches(skill: &Skill, user_text: &str) -> bool {
    let text = user_text.to_lowercase();
    !skill.triggers.is_empty()
        && skill
            .triggers
            .iter()
            .any(|t| text.contains(&t.to_lowercase()))
}

/// 自适应激活：有 triggers 则关键词命中；无 triggers 为模型自选技能，正文常驻。
fn is_active(skill: &Skill, user_text: &str) -> bool {
    if skill.triggers.is_empty() {
        true
    } else {
        matches(skill, user_text)
    }
}

/// 技能索引（注入 system prompt 的常驻部分）：只列名称/描述/触发词，省 token。
pub fn index_prompt(skills: &[Skill]) -> Option<String> {
    if skills.is_empty() {
        return None;
    }
    let mut lines = vec![
        "可用技能（用户输入命中触发词时对应指令已生效，未配置触发词的技能由模型按需采用）："
            .to_string(),
    ];
    for s in skills {
        let trig = if s.triggers.is_empty() {
            String::new()
        } else {
            format!(" [触发词: {}]", s.triggers.join("/"))
        };
        lines.push(format!("- {}: {}{trig}", s.name, s.description));
    }
    Some(lines.join("\n"))
}

/// 本轮生效的技能全文（按加载顺序）。自适应：命中触发词或无触发词的模型自选技能均包含。
pub fn active_bodies(skills: &[Skill], user_text: &str) -> Vec<String> {
    skills
        .iter()
        .filter(|s| is_active(s, user_text))
        .map(|s| format!("## 技能「{}」生效\n{}", s.name, s.body))
        .collect()
}

/// 组装本轮 system prompt 附录：索引常驻（有技能时），生效技能再追加全文。
pub fn prompt_appendix(skills: &[Skill], user_text: &str) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(idx) = index_prompt(skills) {
        parts.push(idx);
    }
    parts.extend(active_bodies(skills, user_text));
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "---\nname: rust-review\ndescription: Rust 代码评审\ntriggers: Review, 代码审查\n---\n1. 检查所有权\n2. 检查错误处理\n";

    #[test]
    fn parses_front_matter_and_body() {
        let s = parse_skill(SAMPLE, "fallback").expect("应解析成功");
        assert_eq!(s.name, "rust-review");
        assert_eq!(s.description, "Rust 代码评审");
        assert_eq!(s.triggers, vec!["Review", "代码审查"]);
        assert!(s.body.contains("检查所有权"));
        assert!(!s.body.contains("---"));
    }

    #[test]
    fn missing_front_matter_falls_back_to_stem() {
        let s = parse_skill("直接正文，没有元数据", "my-skill").expect("应解析成功");
        assert_eq!(s.name, "my-skill");
        assert!(s.triggers.is_empty());
        assert_eq!(s.body, "直接正文，没有元数据");
    }

    #[test]
    fn empty_body_is_rejected() {
        assert!(parse_skill("---\nname: x\n---\n   \n", "x").is_none());
        assert!(parse_skill("", "x").is_none());
    }

    /// 契约核心：未知 front matter 键（如 Claude Code/Codex 的 when_to_use、allowed-tools）必须被忽略。
    #[test]
    fn ignores_unknown_frontmatter_keys() {
        let content = "---\nname: foo\ndescription: 演示\nwhen_to_use: 评审时\nallowed-tools: [Read, Grep]\nagents: openai.yaml\n---\n正文X\n";
        let s = parse_skill(content, "fallback").expect("未知键不应导致失败");
        assert_eq!(s.name, "foo");
        assert_eq!(s.description, "演示");
        assert!(s.body.contains("正文X"));
        assert!(s.triggers.is_empty());
    }

    #[test]
    fn matching_is_case_insensitive_substring() {
        let s = parse_skill(SAMPLE, "f").unwrap();
        assert!(matches(&s, "帮我 review 这段代码"));
        assert!(matches(&s, "请做一次代码审查"));
        assert!(!matches(&s, "写个 hello world"));
        // 无触发词的技能不参与自动匹配。
        let plain = Skill {
            triggers: vec![],
            ..s.clone()
        };
        assert!(!matches(&plain, "review"));
    }

    #[test]
    fn appendix_contains_index_only_until_triggered() {
        let s = parse_skill(SAMPLE, "f").unwrap();
        let skills = vec![s.clone()];
        let idle = prompt_appendix(&skills, "无关任务").expect("有技能就应有索引");
        assert!(idle.contains("rust-review"), "索引应列出技能名");
        assert!(!idle.contains("检查所有权"), "未命中不应带正文");

        let hit = prompt_appendix(&skills, "review 我的代码").expect("命中应有附录");
        assert!(hit.contains("检查所有权"), "命中后应包含技能正文");
    }

    /// 模型自选技能（无 triggers）正文应常驻，无需命中关键词。
    #[test]
    fn triggerless_skill_body_always_injected() {
        let s = parse_skill(
            "name: weekly\ndescription: 周报\n---\n写一份周报\n",
            "weekly",
        )
        .unwrap();
        assert!(s.triggers.is_empty());
        let skills = vec![s];
        let idle = prompt_appendix(&skills, "随便聊聊").expect("有技能就应有索引");
        assert!(idle.contains("写一份周报"), "无触发词语技能正文应常驻");
    }

    #[test]
    fn no_skills_means_no_appendix() {
        assert!(prompt_appendix(&[], "任意").is_none());
        assert!(load_skills(Path::new("/nonexistent-dir-xyz")).0.is_empty());
    }

    #[test]
    fn loads_nested_skill_md_from_dir() {
        let dir = std::env::temp_dir().join(format!("harness_skills_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("b-second")).unwrap();
        std::fs::create_dir_all(dir.join("a-first")).unwrap();
        std::fs::create_dir_all(dir.join("a-first").join("scripts")).unwrap();
        std::fs::write(dir.join("b-second/SKILL.md"), SAMPLE).unwrap();
        std::fs::write(
            dir.join("a-first/SKILL.md"),
            "---\ndescription: 无名技能\ntriggers: foo\n---\n正文A\n",
        )
        .unwrap();
        std::fs::write(dir.join("a-first/scripts/run.sh"), "echo").unwrap();
        std::fs::write(dir.join("ignore.txt"), "不是技能").unwrap();

        let (skills, _warnings) = load_skills(&dir);
        assert_eq!(skills.len(), 2, "应递归找到两个 SKILL.md，忽略非 SKILL.md");
        assert_eq!(
            skills[0].name, "a-first",
            "按 name 排序（缺 name 时回退目录 stem）"
        );
        assert_eq!(skills[1].name, "rust-review");
        // 资源目录探测
        let a = skills.iter().find(|s| s.name == "a-first").unwrap();
        assert_eq!(a.scripts_dir, Some(dir.join("a-first/scripts")));
        let b = skills.iter().find(|s| s.name == "rust-review").unwrap();
        assert_eq!(b.scripts_dir, None, "无 scripts 目录则为 None");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn loads_souls_from_dir() {
        let dir = std::env::temp_dir().join(format!("harness_souls_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("planner")).unwrap();
        std::fs::create_dir_all(dir.join("evaluator")).unwrap();
        std::fs::write(
            dir.join("planner/SOUL.md"),
            "---\nname: planner\ndescription: 规划者\n---\n你是规划者\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("evaluator/SOUL.md"),
            "---\nname: evaluator\ndescription: 评审\n---\n你是评审\n",
        )
        .unwrap();
        let (souls, _warnings) = load_souls(&dir);
        assert_eq!(souls.len(), 2);
        assert_eq!(role_soul(&souls, "planner"), Some("你是规划者"));
        assert_eq!(role_soul(&souls, "missing"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
