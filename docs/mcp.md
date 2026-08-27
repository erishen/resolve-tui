# MCP 配置指南

resolve-tui 内置一个极简 **MCP（Model Context Protocol）stdio 客户端**：
按配置拉起 server 子进程，把远端工具自动合并进模型可用的工具列表；
模型调用时按名字路由回对应 server 执行。全程无需重启 TUI。

---

## 快速开始

### 方式一：TUI 里动态添加（推荐）

```text
/mcp add fs npx -y @modelcontextprotocol/server-filesystem ~/Workspace
```

> 注意 `/mcp add fs` 只写了名字是不够的，完整格式是：
> `/mcp add <名字> <命令> [参数…]`

成功后立即生效（下一轮对话即可用），并追加写入 `config.toml`，重启后仍在。

### 方式二：编辑配置文件

配置文件路径（按顺序取第一个）：

1. 环境变量 `HARNESS_CONFIG`
2. `~/Library/Application Support/resolve-tui/config.toml`（macOS；Linux 为 `~/.config/...`）

```toml
[mcp_servers.fs]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/Users/you/Workspace"]
env = { HTTPS_PROXY = "http://127.0.0.1:7897" }
```

改完后在 TUI 里执行 `/mcp reload`。

## 字段说明

| 字段 | 必填 | 说明 |
|---|---|---|
| `command` | ✅ | 可执行文件（`npx` / `uvx` / 二进制 / 脚本） |
| `args` | | 参数数组 |
| `env` | | 附加环境变量（如代理、API key） |

**server 名约束**：仅字母/数字/`-`/`_`。远端工具暴露名为 `mcp_<server>_<工具名>`
（非法字符转 `_`，总长截断到 64，跨 server 重名先到先得）。

## 命令一览

| 命令 | 作用 |
|---|---|
| `/mcp` | 查看 server 连接状态 |
| `/mcp add <名> <命令> [参数…]` | 动态挂载 + 写入 config.toml |
| `/mcp remove <名>` | 摘除（杀进程、删工具、同步删配置段） |
| `/mcp reload` | 按 config.toml 重连全部 |
| `/tools [on\|off 名]` | 查看/启停工具（含远端，会话级） |

## 运行机制

- **协议**：stdio 上的换行分隔 JSON-RPC 2.0；启动时 `initialize` 握手 → `tools/list`
- **超时**：握手/枚举 15s，工具调用 120s
- **失败隔离**：单个 server 连不上只标记「连接失败」，不影响其它 server 与正常对话
- **审批**：MCP 工具调用与内置工具一样走 y/n 审批（`HARNESS_APPROVE_TOOLS=true`）
- **退出清理**：TUI 退出时自动杀掉所有 server 子进程

---

## 常用 MCP Server 推荐

> 以下均为社区主流选择（npm 包用 `npx -y`，Python 包用 `uvx`）。
> 首次运行会下载包，建议先手动跑一次命令预热缓存（见文末排错）。

### 文件与代码

| Server | 用途 | 添加示例 |
|---|---|---|
| **filesystem**（官方） | 受限目录内读写文件/列目录 | `/mcp add fs npx -y @modelcontextprotocol/server-filesystem ~/Workspace /tmp` |
| **git** | 仓库状态/diff/log/commit | `/mcp add git uvx mcp-server-git --repository ~/Workspace/CNB/individular-invest` |

### 网页与搜索

| Server | 用途 | 添加示例 |
|---|---|---|
| **fetch**（官方） | 抓网页转 Markdown | `/mcp add fetch uvx mcp-server-fetch` |
| **Firecrawl** | 整站爬取/结构化抽取（需 key） | `/mcp add firecrawl npx -y firecrawl-mcp` + env `FIRECRAWL_API_KEY` |
| **exa** | AI 搜索引擎（需 key） | `/mcp add exa npx -y exa-mcp-server` + env `EXA_API_KEY` |

### 浏览器自动化

| Server | 用途 | 添加示例 |
|---|---|---|
| **Playwright**（微软官方） | 无头浏览器操作/截图/E2E | `/mcp add pw npx -y @playwright/mcp@latest` |
| **Chrome DevTools MCP**（Google 官方，下载量第一） | 接管真实 Chrome 调试页面/性能 | `/mcp add cdp npx -y chrome-devtools-mcp` |

### 数据库

| Server | 用途 | 添加示例 |
|---|---|---|
| **SQLite**（官方） | 查询本地库 | `/mcp add sqlite uvx mcp-server-sqlite --db-path ./data.db` |
| **MongoDB** | 查询 Mongo | `/mcp add mongo npx -y mongodb-mcp-server` + 连接串 env |
| **ClickHouse** | OLAP 查询 | `/mcp add ch uvx mcp-clickhouse` |

### 记忆与推理增强

| Server | 用途 | 添加示例 |
|---|---|---|
| **memory**（官方） | 跨会话知识图谱记忆 | `/mcp add mem npx -y @modelcontextprotocol/server-memory` |
| **sequential-thinking**（官方） | 结构化分步推理 | `/mcp add think npx -y @modelcontextprotocol/server-sequential-thinking` |
| **Context7** | 实时拉取各类库的最新文档 | `/mcp add ctx7 npx -y @upstash/context7-mcp` |

### 开发者工具 / 云

| Server | 用途 | 添加示例 |
|---|---|---|
| **Sentry** | 查错误事件/堆栈 | `/mcp add sentry npx -y @sentry/mcp-server` |
| **Azure MCP**（微软官方） | 管理 Azure 资源 | `/mcp add azure npx -y @azure/mcp` |
| **GitHub** | 仓库/Issue/PR 操作（需 token） | `/mcp add gh npx -y @modelcontextprotocol/server-github` + env `GITHUB_TOKEN` |

### 其它

| Server | 用途 | 添加示例 |
|---|---|---|
| **time**（官方） | 时区/时间换算 | `/mcp add time uvx mcp-server-time` |
| **everything**（官方） | 全功能演示，客户端联调用 | `/mcp add demo npx -y @modelcontextprotocol/server-everything` |

> 更多可浏览官方 registry：<https://registry.modelcontextprotocol.io/>

带 API key 的写法（config.toml 中）：

```toml
[mcp_servers.firecrawl]
command = "npx"
args = ["-y", "firecrawl-mcp"]
env = { FIRECRAWL_API_KEY = "fc-xxx", HTTPS_PROXY = "http://127.0.0.1:7897" }
```

> `/mcp add` 目前不支持 env 参数；需要环境变量的 server 请先写入 config.toml 再 `/mcp reload`。

---

## 安全须知

- **沙箱不覆盖 MCP 进程**：内置 shell/write_file 有白名单沙箱，
  但 MCP server 是独立进程，权限取决于它自己（如 filesystem 的目录白名单由你传入的 args 决定）。
  **只添加可信来源的 server**。
- 所有工具调用默认经人工审批后才执行。
- `env` 里的 token 会明文存在 config.toml，注意文件权限。

## 故障排查

| 现象 | 处理 |
|---|---|
| 「连接失败」且是首次用 npx | 先在终端手动跑一次同样命令让 npm 把包装好（握手超时 15s 可能不够下载） |
| `/mcp` 显示已连接但模型不用 | 新工具从**下一轮对话**开始生效；确认没有被 `/tools off` 禁用 |
| 工具名冲突被忽略 | 日志提示「工具暴露名冲突」；给 server 换个名字即可 |
| 修改 config.toml 后没变化 | 执行 `/mcp reload`；或检查 `HARNESS_CONFIG` 是否指向了别的文件 |
