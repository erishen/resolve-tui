# 工具使用示例

启动后直接把下面这些话输入 TUI 即可。`❯` 后为输入内容，无需原样照抄——
模型会自己决定调用哪个工具、调用几次。

> 审批模式（`HARNESS_APPROVE_TOOLS=true`）下，每次工具调用会先弹出 y/n 确认。
> 想看模型实际调了什么：观察历史里的 `→ 调用 xxx (call_…)` 行，或随时 `/tools`。

---

## 一、内置工具

### shell —— 执行命令

```text
❯ 看看当前目录是什么 git 仓库，最近 5 条提交都改了什么
```

```text
❯ 统计一下 src/ 下各语言的代码行数，按行数排序给我个表格
```

```text
❯ cargo test -p resolve-tui 跑一遍，如果有失败的先分析原因再尝试修复
```

```text
❯ 用 curl 探测 https://apihub.agnes-ai.com 的连通性并测一下延迟
```

### read_file / list_dir —— 读文件、浏览目录

```text
❯ 这个项目的入口在哪？从 main.rs 开始帮我梳理一次请求的完整调用链路
```

```text
❯ 对比 resolve-tui/src/agent.rs 和 src/tui/app.rs 的职责划分是否清晰，有没有该挪的代码
```

### write_file —— 写文件

```text
❯ 把刚才的分析结论写成 docs/architecture.md，包含模块依赖图（mermaid）
```

```text
❯ 给 skills 目录新增一个「SQL 优化」技能文件，触发词 sql、慢查询
```

---

## 二、MCP 远端工具

远端工具名形如 `mcp_<server>_<工具>`，模型自动选用，你只管提需求。

### fetch —— 抓取网页（转 Markdown）

```text
❯ 帮忙看看 erishen.cn 这个网站啥内容
```

```text
❯ 抓取 https://docs.rs/ratatui 最新版本的 List widget 文档，总结常用方法
```

```text
❯ 对比抓取这两篇文章的核心观点：URL1 和 URL2
```

### filesystem server —— 大目录检索 / 跨项目读文件

```text
❯ 在 ~/Workspace/CNB 里找所有包含 "TODO: 性能" 的 Rust 文件，列个清单
```

```text
❯ 把 individular-invest 和其它三个项目的 README 标题结构对比一下
```

---

## 三、组合任务（多轮工具链）

模型会在一次任务里串联多个工具——这正是 agent 循环的价值：

```text
❯ 全面体检这个仓库：
  1. 列出目录结构
  2. 找出超过 500 行的源码文件
  3. 检查有没有 unwrap()/panic! 滥用
  4. 最后给一份按优先级排序的重构建议表
```

```text
❯ 读 work/rust/Cargo.toml 的依赖列表，逐个去 crates.io / 官方文档确认最新版本，
  输出「当前版本 → 最新版本 → 是否有 breaking」的表格；能升级的顺手改 Cargo.toml
```

```text
❯ 复现一个 bug：写个小脚本制造并发写入冲突，运行它，根据报错定位到代码行，
  给出最小修复 patch 并验证修复后脚本通过
```

```text
❯ 把 sessions/ 目录里所有 last.json 的对话统计一下：总轮数、平均回答长度，
  存成 report.csv
```

---

## 四、技能触发

`.resolve-tui-skills/*.md` 中触发词命中的技能全文会注入当轮：

| 你说 | 触发的技能 |
|---|---|
| `❯ 帮我 review 这段代码：…` | rust-review（triggers: review/代码审查） |
| `❯ 做一次完整的代码审查，重点是错误处理` | 同上 |

新增技能：在技能目录放一个带 front matter 的 `.md`，然后 `/skills reload`。

---

## 五、控制技巧

| 需求 | 做法 |
|---|---|
| 不想让模型动某个工具 | `/tools off shell`（本轮会话内生效） |
| 每次工具调用都要人工确认 | 启动前 `HARNESS_APPROVE_TOOLS=true` |
| 控制 token 花费 | `HARNESS_MAX_TOKENS=50000`（超出终止循环） |
| 强制模型至少用一个工具（调试） | `HARNESS_FORCE_TOOLS=true` |
| 任务跑偏想中止 | 运行中按 Esc，直接说新的指令即可 |
