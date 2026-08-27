# Agent-Harness 仓库全面体检报告

> 生成时间：2026-08-26（基于实际代码扫描，非推测）
> 重构收尾：2026-08-27（见文末「本次重构完成情况」）

## 1. 目录结构

```
resolve-tui/
├── Cargo.toml              # Rust 2024 edition, 依赖见 section 2 注释
├── Makefile                # build/run/watch/dev/test 命令（已静音 cargo 编译噪声）
├── .env / .env.example     # 环境变量模板（OPENAI_API_BASE / HARNESS_MODEL / 代理）
├── docs/
│   ├── examples.md         # 工具使用示例
│   ├── mcp.md              # MCP 协议与 /mcp add 示例
│   └── audit-report.md     # 本体检报告
├── src/
│   ├── main.rs             # 入口：TUI + HTTP 服务器 + panic hook 恢复终端
│   ├── lib.rs              # 模块声明
│   ├── config.rs           # 配置：env + config.toml 合并（表驱动），MCP 持久化
│   ├── error.rs            # HarnessError 定义
│   ├── model.rs            # 消息/请求/响应数据结构（Responses API）
│   ├── llm.rs              # LLM 交互（流式/非流式 + SSE 解析）
│   ├── agent.rs            # Agent 核心逻辑调度（drive 循环、工具调用、审批）
│   ├── codegen/            # 运行时代码生成（fastpath 未命中时的 rhai 检测器）
│   │   ├── mod.rs          # 纯 re-export 枢纽 + 公开入口
│   │   ├── engine.rs       # 受限 rhai 引擎 + 正则护栏
│   │   ├── extract.rs      # 检测器源码抽取
│   │   ├── sandbox.rs      # 隔离执行（子进程协议 / 超时 kill / 逐源）
│   │   └── plugins.rs      # 插件缓存/统计/治理/持久化/主流程
│   ├── fastpath/           # 确定性快路径（零模型纯代码求解）
│   │   ├── mod.rs          # FastAnswer + 共享工具 + try_fast_answer 分发
│   │   ├── arithmetic.rs   # 安全算术
│   │   ├── statistics.rs   # 数字统计（含 false-positive 收紧）
│   │   ├── date.rs         # 日期计算
│   │   ├── unit.rs         # 单位换算
│   │   ├── base.rs         # 进制转换
│   │   └── time.rs         # 当前时间
│   ├── mcp/                # MCP 协议客户端（stdio JSON-RPC）
│   │   ├── mod.rs          # 对外 API + 运行时增删（RwLock）
│   │   ├── client.rs       # 单 server 生命周期（spawn/stdin）
│   │   ├── manager.rs      # 多 server 管理
│   │   └── protocol.rs     # jsonrpc 原语
│   ├── tools.rs            # 内置沙箱工具集 + truncate_output
│   ├── sandbox.rs          # 命令沙箱执行器（限根/网络开关）
│   ├── sessions.rs         # 会话管理（list/resolve/delete，存为 JSON）
│   ├── skills.rs           # Skills 系统（front matter .md，触发词注入）
│   ├── memory.rs           # 长期记忆（AGENT.md / MEMORY.md / /remember）
│   ├── agent_tests.rs      # Agent 集成测试
│   ├── model_tests.rs      # Model 测试
│   └── tui/
│       ├── mod.rs          # run_tui 主循环、agent 任务接线
│       ├── app.rs          # App 状态 + 事件处理（on_event）
│       ├── format.rs        # 纯文本格式化（行内 Markdown / GFM 表格 / 能力面 / 显示宽）
│       ├── input.rs         # 键盘事件 + Tab 补全 + 审批应答
│       ├── input/
│       │   └── input_tests.rs  # 输入/命令集成测试
│       ├── commands/        # 会话控制命令（/list /create /apply …）
│       │   └── mod.rs      # handle_control 解析 + 16 个 cmd_* 处理函数
│       ├── render.rs       # 布局渲染与折行（hang 悬挂缩进）— 见注
│       ├── theme.rs        # 配色主题 + OSC 11 终端背景探测
│       ├── util.rs         # 剪贴板 / 路径显示 / 参数美化 / truncate_ellipsis
│       └── wrap.rs         # 文本折行
└── .resolve-tui-skills/rust-review.md   # 内置示例技能
```

- **crate 类型**：`lib + bin`
- **模块分层**：fastpath（纯代码）→ codegen（rhai 沙箱生成）→ agent（LLM 循环）三级级联；tui / mcp / skills 为外围支撑。

---

## 2. 超长文件（> 500 行）

| 文件 | 行数 | 说明 |
|------|------|------|
| `src/tui/app.rs` | **784** | App 状态机 + 事件处理（已从 973 行抽出 format.rs） |
| `src/agent.rs` | **692** | Agent 核心调度（drive 单函数较长） |
| `src/tui/render.rs` | **621** | TUI 渲染与折行 |

> 已拆分（行为不变，测试全绿）：
> - `codegen.rs`（原 1093 行）→ `src/codegen/` 5 文件（mod 27 / engine 99 / extract 108 / sandbox 296 / plugins 630）
> - `fastpath.rs`（原 956 行）→ `src/fastpath/` 7 文件（mod 263 + 6 匹配器）
> - `tui/input.rs`（原 668 行）→ `input.rs` 228 + `tui/commands/` 521（16 个 cmd_* 函数）
> - `mcp.rs`（原 542 行）→ `src/mcp/` 4 文件
> - `config.rs` 的 `apply_env` / `apply_toml` 改为 `&[Box<dyn Fn>]` 表驱动

---

## 3. unwrap()/panic! 检查（实际扫描结果）

### 统计（仅非测试生产代码）

| 类型 | 数量 | 风险 |
|------|------|------|
| `unwrap_or_*` / `unwrap_or_else` | ~20 | **低**（全部是安全默认值或 poison 恢复） |
| `expect()` | 1 | 已修复（见下） |
| `panic!()` | 0（生产） | — |

**关键结论**：早期一次失败任务生成的旧报告声称「`Config::load()` 启动即 panic、66 处危险 unwrap」——**与现状不符**。`config.rs::load()` 早已改为返回 `Arc<Config>` 且用 `unwrap_or_default()` 读取可选环境变量，不存在启动 panic。

### 唯一真实隐患（已修复）

- `agent.rs` 旧代码 `let mgr = self.mcp.as_ref().expect("上一行刚初始化");`
  改为 `match &self.mcp { Some(m) => m.clone(), None => { 现场建 Arc 后 clone } }`，
  彻底消除「理论上的 panic 路径」（逻辑上本不可达，但去掉了断言式 expect）。

### 其余 unwrap 分布（均为安全默认值，非崩溃点）

| 文件 | 用法 |
|------|------|
| `agent.rs` | `approval_rx.recv().await.map(...).unwrap_or(false)`、`model.lock().unwrap_or_else(|e| e.into_inner())`（poison 恢复） |
| `llm.rs` | `resp.text().await.unwrap_or_default()`、`reqwest::Client::new()` 兜底 |
| `mcp/` | `serde_json::from_str(...).unwrap_or(Value::Null)`（参数解析失败给空对象） |
| `model.rs` | `serde_json::to_string(&other).unwrap_or_else(|_| "{}".into())`（序列化兜底） |
| `app.rs` / `render.rs` | `model.lock().unwrap_or_else(|e| e.into_inner())`（poison 恢复） |
| `config.rs` | `std::env::var(...).unwrap_or_default()` |

---

## 4. 重构建议（按优先级排序）

### 🔥 P0 — 崩溃/健壮性（已基本清零）

| # | 问题 | 状态 |
|---|------|------|
| 1 | `mcp.as_ref().expect` 理论 panic | ✅ 已改为 match 守卫 |
| 2 | MCP 工具结果超长导致模型连接中断 | ✅ 已统一 `truncate_output`（≤8K 字符） |

### ⚡ P1 — 高优先级（可维护性）

| # | 问题 | 建议 | 涉及文件 | 状态 |
|---|------|------|----------|------|
| 3 | 输入/命令处理过长 | 测试迁独立文件 + 命令拆 `commands/` | `src/tui/input.rs` | ✅ 已完成 |
| 4 | `agent.rs` 692 行，drive 单函数过长 | 抽出 `execute_tool_call` / `handle_completed` / `accumulate` 子函数 | `src/agent.rs` | ⬜ 待做 |
| 5 | `tui/render.rs` 621 行 | 按区域拆 `render_header` / `render_chat` / `render_footer` | `src/tui/render.rs` | ⬜ 待做 |
| 6 | `mcp.rs` 542 行 | 拆 `protocol` / `client` / `manager` | `src/mcp.rs` | ✅ 已完成 |

### 📋 P2 — 中优先级（校验/工程）

| # | 问题 | 建议 | 状态 |
|---|------|------|------|
| 7 | `config.rs` 无配置校验 | 加 `validate()`：校验 `api_base` 是合法 URL、`model` 非空、端口范围 | ✅ 已有 `validate()` |
| 8 | 错误类型分散 | 统一用 `thiserror` 定义 project error，复用 `error.rs` | ⬜ 待做 |
| 9 | 缺少集成测试 | 补流式响应中断恢复、MCP 工具超时隔离的集成测试 | ⬜ 待做 |
| 10 | `config.rs` 覆盖逻辑样板 | `apply_env` / `apply_toml` 改为表驱动 | ✅ 已完成 |

### 💡 P3 — 低优先级（长期）

| # | 建议 |
|---|------|
| 11 | 配置 `.rustfmt.toml` + `cargo clippy --all-targets -- -D warnings` 进 CI |
| 12 | 关键公开函数补 doc comments |
| 13 | API key 走系统 keychain / 加密 env，而非明文 `.env` |

---

## 总结

**项目状态**：功能完整、架构清晰（Agent / TUI / MCP / Skills 四层分离），**无启动期崩溃风险**。三级级联（fastpath → codegen → agent）使判定性问题零模型开销、结构化小任务走便宜模型。

**本次体检发现并修复**：
1. `app.rs` 纯函数抽到 `tui/format.rs`（973 → 784 行，更易维护）
2. `agent.rs` 消除唯一理论 panic 路径

**2026-08-27 重构收尾（行为零变化，始终 109 测试 + clippy/fmt + 四 feature 组合全绿）**：
1. `codegen.rs`（1093 行）→ `src/codegen/` 5 文件，插件缓存锁改为 poison 可恢复，补 GitHub Actions CI + 双重编译修复
2. `fastpath.rs`（956 行）→ `src/fastpath/` 7 文件（每匹配器独立 + 共享工具）
3. `tui/input.rs`（668 行）→ `input.rs` + `tui/commands/`（handle_control 拆 16 个 cmd_* 函数）
4. `mcp.rs`（542 行）→ `src/mcp/` 4 文件（mod/client/manager/protocol）
5. `config.rs` 的 `apply_env` / `apply_toml` 改为表驱动
6. `llm.rs` 系统提示工具清单改为从 `builtin_tools()` 动态生成（防漂移）

**核心建议**：剩余可维护性重点是 `agent.rs`（drive 循环）与 `tui/render.rs` 两个 >500 行文件按 P1 方案拆分；错误处理层面已无需紧急改动。
