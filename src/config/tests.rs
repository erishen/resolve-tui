//! `Config` 的单元测试：合并优先级、MCP 段持久化、校验、API key 解析。

use super::*;

// 默认值 → 配置文件 → 环境变量：验证文件能覆盖默认、环境变量再覆盖文件；
// 合并为单测以避免并行修改进程级环境变量造成的竞态。
#[test]
fn load_merges_file_then_env() {
    let dir = std::env::temp_dir().join("harness_cfg_test");
    let _ = std::fs::create_dir_all(&dir);
    let toml_path = dir.join("config.toml");
    std::fs::write(
        &toml_path,
        r#"
model = "agnes-x"
max_iterations = 8
approve_tools = true
"#,
    )
    .unwrap();

    let prev_model = std::env::var("HARNESS_MODEL").ok();
    unsafe {
        std::env::set_var("HARNESS_CONFIG", &toml_path);
        std::env::set_var("HARNESS_MODEL", "env-model");
    }

    let cfg = Config::load();

    // 文件覆盖了默认 model 与 max_iterations；环境变量再覆盖文件。
    assert_eq!(cfg.model, "env-model", "环境变量应覆盖文件");
    assert_eq!(cfg.max_iterations, 8, "文件应覆盖默认值");
    assert!(cfg.approve_tools, "文件里的开关应生效");
    // 未出现在文件/环境变量中的项保持默认。
    assert_eq!(cfg.api_base, "https://api.openai.com/v1");

    // 还原环境变量，避免影响其它测试。
    unsafe {
        std::env::remove_var("HARNESS_CONFIG");
        match prev_model {
            Some(v) => std::env::set_var("HARNESS_MODEL", v),
            None => std::env::remove_var("HARNESS_MODEL"),
        }
    }
    let _ = std::fs::remove_dir_all(&dir);

    // 缺失配置文件、且无环境变量覆盖时，应退化到内置默认值。
    unsafe {
        std::env::remove_var("HARNESS_CONFIG");
        std::env::remove_var("HARNESS_MODEL");
    }
    let cfg2 = Config::load();
    assert_eq!(cfg2.model, "gpt-4o-mini");
    assert_eq!(cfg2.max_iterations, 16);
}

#[test]
fn toml_mcp_servers_are_parsed_in_order() {
    let t: TomlConfig = toml::from_str(
        r#"
[mcp_servers.filesystem]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]

[mcp_servers.fetch]
command = "uvx"
args = ["mcp-server-fetch"]
env = { HTTP_PROXY = "http://127.0.0.1:7890" }
"#,
    )
    .unwrap();

    let mut cfg = Config::defaults();
    cfg.apply_toml(&t);

    assert_eq!(cfg.mcp_servers.len(), 2, "两个 server 都应被解析");
    // BTreeMap 按名字排序，保证启动顺序确定。
    assert_eq!(cfg.mcp_servers[0].name, "fetch");
    assert_eq!(cfg.mcp_servers[1].name, "filesystem");
    let fs = &cfg.mcp_servers[1];
    assert_eq!(fs.command, "npx");
    assert_eq!(
        fs.args,
        vec!["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]
    );
    assert!(fs.env.is_empty(), "缺省 env 应为空表");
    assert_eq!(
        cfg.mcp_servers[0].env.get("HTTP_PROXY").map(String::as_str),
        Some("http://127.0.0.1:7890")
    );
}

// /mcp add|remove 的持久化：追加保留原有内容；重名拒绝；删除只摘自己的段。
#[test]
fn append_and_remove_mcp_server_sections() {
    let path = std::env::temp_dir().join(format!("harness_mcp_cfg_{}.toml", std::process::id()));
    let _ = std::fs::remove_file(&path);
    std::fs::write(
        &path,
        "# 我的配置\nmodel = \"m1\"\n\n[mcp_servers.old]\ncommand = \"old-cmd\"\n",
    )
    .unwrap();

    // 追加：带参数与特殊字符（引号/反斜杠）转义。
    crate::config::append_mcp_server(
        &path,
        "new-srv",
        "npx",
        &["-y".into(), "@scope/pkg \"v1\"".into()],
    )
    .expect("append 应成功");

    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.starts_with("# 我的配置"), "原注释应保留");
    assert!(text.contains("[mcp_servers.new-srv]"));
    assert!(
        text.contains("\"@scope/pkg \\\"v1\\\"\""),
        "应正确转义: {text}"
    );

    // 追加后整体仍能被 TomlConfig 解析，且两个 server 都在（BTreeMap 按名排序）。
    let parsed: TomlConfig = toml::from_str(&text).unwrap();
    let names: Vec<&String> = parsed.mcp_servers.as_ref().unwrap().keys().collect();
    assert_eq!(names, vec!["new-srv", "old"]);

    // 重名追加必须拒绝。
    assert!(crate::config::append_mcp_server(&path, "old", "x", &[]).is_err());

    // 非法名字拒绝。
    assert!(crate::config::append_mcp_server(&path, "bad.name", "x", &[]).is_err());

    // 删除 new-srv：old 段与头部注释保留。
    assert!(crate::config::remove_mcp_server(&path, "new-srv").unwrap());
    let text2 = std::fs::read_to_string(&path).unwrap();
    assert!(!text2.contains("new-srv"));
    assert!(text2.contains("[mcp_servers.old]"));
    assert!(text2.contains("# 我的配置"));

    // 再删一次返回 false；删除不存在的文件返回 false 而非报错。
    assert!(!crate::config::remove_mcp_server(&path, "new-srv").unwrap());
    let missing = std::env::temp_dir().join("harness_no_such_cfg.toml");
    let _ = std::fs::remove_file(&missing);
    assert!(!crate::config::remove_mcp_server(&missing, "x").unwrap());

    let _ = std::fs::remove_file(&path);
}

#[test]
fn validate_accepts_sane_config() {
    let cfg = Config::defaults();
    assert!(
        cfg.validate().is_ok(),
        "默认值应通过校验: {:?}",
        cfg.api_base
    );
}

#[test]
fn validate_rejects_bad_values() {
    let mut cfg = Config::defaults();
    cfg.model = String::new();
    assert!(cfg.validate().is_err());

    let mut cfg = Config::defaults();
    cfg.api_base = "ftp://example.com".to_string();
    assert!(cfg.validate().is_err());

    let mut cfg = Config::defaults();
    cfg.api_base = "https://api.example.com/".to_string();
    assert!(cfg.validate().is_err(), "结尾斜杠应被拒绝");

    let mut cfg = Config::defaults();
    cfg.max_iterations = 0;
    assert!(cfg.validate().is_err());

    let mut cfg = Config::defaults();
    cfg.theme = "neon".to_string();
    assert!(cfg.validate().is_err());
}

#[test]
fn resolve_api_key_prefers_env_over_keyring() {
    // 环境变量优先级最高：即使钥匙串里有值，也应返回环境变量的值。
    unsafe {
        std::env::set_var("OPENAI_API_KEY", "env-secret");
    }
    assert_eq!(Config::resolve_api_key(), "env-secret");
    unsafe {
        std::env::remove_var("OPENAI_API_KEY");
    }
}
