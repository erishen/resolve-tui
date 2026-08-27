# resolve-tui

CLI / TUI 双形态的编码 agent：把「快路径 + codegen 缓存 + LLM 主循环」串成三级级联，
支持 Agent Skills 技能包、可选多 Agent（Planner/Specialist/Evaluator 三角色）编排、
MCP 服务器集成，以及带每任务独立工作区的沙箱。

## 功能特性

- **三级响应链**：fastpath（零模型）→ codegen（缓存命中）→ LLM 主循环（流式对话）
- **Agent Skills**：对齐 Agent Skills 开放标准，通过 resolve-skills submodule 提供技能包
- **PSE 三角色**：Planner / Specialist / Evaluator 多 Agent 编排，独立验证门禁
- **沙箱隔离**：每任务独立工作区，macOS `sandbox-exec` / Linux `bwrap` 隔离，默认断网
- **MCP 支持**：动态添加/移除 MCP 服务器，扩展工具能力
- **会话与记忆**：退出自动存档、启动自动续接，跨会话长期记忆
- **CLI / TUI 双形态**：单次任务命令行执行，或交互式终端界面

## 快速开始

```bash
# 克隆仓库（含 submodule）
git clone --recursive https://github.com/erishen/resolve-tui.git
cd resolve-tui

# 如已克隆但未初始化 submodule：
git submodule update --init --recursive

cp .env.example .env      # 填写 OPENAI_API_KEY；可改 OPENAI_API_BASE / HARNESS_MODEL
cargo build               # 默认特性 tui,codegen

./target/debug/resolve-tui "计算 12345 * 6789"          # 单次任务（CLI）
./target/debug/resolve-tui --tui                        # 交互界面
./target/debug/resolve-tui --multi-agent "实现 xxx"      # PSE 三角色多 Agent
```

`.env` 由程序自动加载（crate 根目录内置 dotenv 支持）。

> **技能包**：本仓库通过 git submodule 引入 [`resolve-skills`](https://github.com/erishen/resolve-skills)，
> 提供 code-review / post-comment / rust-review / weekly-investment-review 等技能，以及 PSE 三角色人格。
> submodule 路径 `./resolve-skills/`，技能目录 `./resolve-skills/skills/`，人格目录 `./resolve-skills/souls/`。

## 三级响应链

1. **fastpath**：算术/单位/日期等纯代码可解问题直接短路，零模型往返。
2. **codegen**：命中已学习插件（确定性任务）零模型返回；检测器生成走分层便宜模型。
3. **LLM 主循环**：Responses API 流式对话，内置 `shell / read_file / write_file / list_dir` 工具。

## 技能（Agent Skills 对齐）

- 技能即 `<skill>/SKILL.md` 提示词包（frontmatter：`name`/`description`/`triggers`），
  遵循 `resolve-skills` 仓库的 SKILL_SPEC 契约，可被 Claude Code / Codex 零改动消费。
- 搜索顺序：`$HARNESS_SKILLS_DIR` → `<cwd>/.resolve-tui-skills/` → 内置 submodule。
- 自适应激活：有 `triggers` 命中注入正文，无则常驻模型自选。

## 多 Agent（PSE）模式

三角色按 `agentic-souls` 思路编排（角色定义在 `resolve-skills/souls/`）：

- **Planner**（主 agent）：规划分解，仅 `delegate_specialist` + `evaluate` + 只读工具。
- **Specialist**（子循环）：执行（含写），完整工具。
- **Evaluator**（子循环）：独立验证，只读工具，输出 PASS/PARTIAL/FAIL/BLOCKED。

开启：`--multi-agent` / `HARNESS_MULTI_AGENT=1` 等启动项，或 TUI 内 `/pse on|off`（下一轮生效）。

## MCP 支持

- 动态添加 MCP 服务器：`/mcp add <name> <command> [args...]`
- 移除 MCP 服务器：`/mcp remove <name>`
- 列出已配置的 MCP 服务器：`/mcp list`
- 支持 stdio 传输的 MCP 服务器（如 filesystem、GitHub、数据库等）

示例：
```bash
# 添加文件系统 MCP 服务器
/mcp add fs npx -y @modelcontextprotocol/server-filesystem /path/to/dir

# 添加 GitHub MCP 服务器
/mcp add gh npx -y @modelcontextprotocol/server-github
```

> 注意：MCP 服务器的 `env` 配置会明文存在 `config.toml`，注意文件权限。

## 沙箱

- 默认根目录 `<项目>/.resolve-tui-sandbox/`（`HARNESS_SANDBOX_DIR` 覆盖），启动即创建。
- **每任务独立工作区** `<root>/task-<nanos>-<pid>/`：`write_file`/shell 的相对路径与产物全部落
  在该工作区，任务间互不覆盖；workdir 与可写白名单即工作区。
- **读范围**：项目目录 + 工作区 + 系统临时目录（`read_file`/`list_dir`），盘外文件拒绝——
  防本地敏感文件（`~/.ssh` 等）被读进上下文外发。
- shell 经 macOS `sandbox-exec` / Linux `bwrap` 隔离：默认断网，写限白名单。
- 启动时自动清理超过 7 天的旧工作区（`HARNESS_SANDBOX_DIR` 根下）。
- 系统提示会注入「当前可写工作区 + 白名单 + 读取范围」，模型不再瞎猜路径。

## 会话与记忆

- 退出自动存档、启动自动续接；`/list /create /apply /save /load /clear /rm` 管理。
- 会话目录 `.resolve-tui-sessions/`（已忽略入库，落盘 0600）。
- `/remember` 跨会话长期记忆，`MEMORY.md` 存系统配置目录（0600）。

## 环境变量

| 变量 | 说明 | 默认 |
|---|---|---|
| `OPENAI_API_KEY` | API key（可走 macOS Keychain） | — |
| `OPENAI_API_BASE` | OpenAI 兼容 base（不含 `/responses`） | `https://api.openai.com/v1` |
| `HARNESS_MODEL` | 主模型 | `gpt-4o-mini` |
| `HARNESS_CODEGEN` / `_MODEL` / `_DIR` | codegen 开关 / 便宜模型 / 插件目录 | on / 跟随主模型 |
| `HARNESS_SANDBOX` / `_ALLOW_NETWORK` / `_ROOTS` / `_DIR` | 沙箱开关 / 网络 / 写白名单 / 根目录 | on / off / cwd+temp |
| `HARNESS_MULTI_AGENT` | 是否默认 PSE 多 Agent | off |
| `HARNESS_MAX_ITERATIONS` / `_MAX_TOKENS` / `_HISTORY_MAX_ITEMS` | 迭代 / token 预算 / 历史窗 | 16 / 不限 / 200 |
| `HARNESS_SKILLS_DIR` / `PSE_SOULS_DIR` | 技能 / PSE 三角色人格目录 | `./resolve-skills/skills` / `./resolve-skills/souls` |
| `HARNESS_SESSIONS_DIR` | 会话目录 | `.resolve-tui-sessions` |
| `HARNESS_STATEFUL` / `_FORCE_TOOLS` / `_APPROVE_TOOLS` / `_THEME` | 有状态续接 / 强制工具 / 审批 / 主题 | off |

## TUI 命令

`/pse [on|off]` · `/sandbox [clean]` · `/model [名]` · `/skills [reload]` · `/mcp add|remove|list`
· `/tools [on|off 名]` · `/remember` · `/list /create /apply /save /load /clear /rm` · `/export`
· `/reasoning` · `/examples` · `/help`

## 构建与测试

```bash
# 构建
cargo build

# 运行测试
cargo test

# 代码格式化
cargo fmt

# 代码检查
cargo clippy
```

## 文档

- [`docs/architecture.md`](./docs/architecture.md) — 架构设计文档（三级响应链 / PSE 三角色 / 沙箱 / MCP）
- [`docs/examples.md`](./docs/examples.md) — 使用示例与常见问题
- [`docs/mcp.md`](./docs/mcp.md) — MCP 服务器配置与使用
- [`docs/audit-report.md`](./docs/audit-report.md) — 安全审计报告

## 隐私姿态

- 不采集遥测；除调用所选 LLM API 外不向任何第三方发数据。
- 密钥仅存环境变量 / 系统钥匙串，不落 URL、不进日志、不随改动提交（`.env` 已忽略）。
- 会话、记忆、沙箱工作区均不入库；敏感文件落盘 0600。
- 边界提示：任务全文、注入的 `AGENT.md`/`MEMORY.md`、工具参数与命令输出会随 LLM 请求外发，
  不要把绝密内容喂给模型。

## 项目结构

```
resolve-tui/
├── resolve-skills/     # git submodule: 技能包 + PSE 三角色人格
│   ├── skills/         #   code-review / post-comment / rust-review / weekly-investment-review
│   └── souls/          #   planner / specialist / evaluator
├── src/
│   ├── agent/          # PSE 编排（roles）与主循环（drive/helpers）
│   ├── codegen/        # codegen 插件与隔离检测器沙箱
│   ├── config/         # 配置（TOML + env 表驱动合并）
│   ├── fastpath/       # 确定性快路径
│   ├── llm.rs          # Responses API 客户端（流式/重试/用量）
│   ├── memory.rs       # 跨会话长期记忆
│   ├── sandbox.rs      # 沙箱策略 / 任务工作区 / seatbelt·bwrap
│   ├── sessions.rs     # 会话目录管理
│   ├── skills.rs       # SKILL_SPEC 技能加载器（含 souls）
│   └── tui/            # interactive UI（App / render / commands）
├── docs/               # 文档（examples / mcp / audit-report）
├── examples/           # 代码示例
├── tests/              # 集成测试
├── .env.example        # 环境变量模板
├── .gitmodules         # submodule 配置
└── Cargo.toml
```

## 许可证

MIT
