# resolve-tui

CLI / TUI dual-mode coding agent: a three-tier cascade of **fastpath + codegen cache + LLM main loop**,
with Agent Skills support, optional multi-agent (Planner/Specialist/Evaluator) orchestration,
MCP server integration, and per-task isolated sandbox workspaces.

## Features

- **Three-tier response chain**: fastpath (zero-model) → codegen (cache hit) → LLM main loop (streaming)
- **Agent Skills**: aligned with the Agent Skills open standard, skills provided via resolve-skills submodule
- **PSE tri-role**: Planner / Specialist / Evaluator multi-agent orchestration with independent verification gate
- **Sandbox isolation**: per-task independent workspaces, macOS `sandbox-exec` / Linux `bwrap` isolation, network disabled by default
- **MCP support**: dynamically add/remove MCP servers to extend tool capabilities
- **Sessions & memory**: auto-save on exit, auto-resume on start, cross-session long-term memory
- **CLI / TUI dual-mode**: single-task command line execution, or interactive terminal interface

## Quick Start

```bash
# Clone with submodules
git clone --recursive https://github.com/erishen/resolve-tui.git
cd resolve-tui

# If already cloned without submodules:
git submodule update --init --recursive

cp .env.example .env      # Fill in OPENAI_API_KEY; optionally change OPENAI_API_BASE / HARNESS_MODEL
cargo build               # Default features: tui,codegen

./target/debug/resolve-tui "calculate 12345 * 6789"          # Single task (CLI)
./target/debug/resolve-tui --tui                              # Interactive UI
./target/debug/resolve-tui --multi-agent "implement xxx"      # PSE tri-role multi-agent
```

`.env` is loaded automatically (built-in dotenv support at crate root).

> **Skills pack**: This repo includes [`resolve-skills`](https://github.com/erishen/resolve-skills) as a git submodule,
> providing skills like code-review / post-comment / rust-review / weekly-investment-review, plus PSE tri-role souls.
> Submodule path: `./resolve-skills/`, skills dir: `./resolve-skills/skills/`, souls dir: `./resolve-skills/souls/`.

## Three-Tier Response Chain

1. **fastpath**: Pure-code solvable problems (arithmetic/unit/date) short-circuit directly, zero model round-trip.
2. **codegen**: Deterministic tasks with learned plugins return with zero model calls; new task detection uses a tiered cheap model.
3. **LLM main loop**: Streaming conversation via Responses API, with built-in `shell / read_file / write_file / list_dir` tools.

## Skills (Agent Skills aligned)

- A skill is a `<skill>/SKILL.md` prompt pack (frontmatter: `name`/`description`/`triggers`),
  following the SKILL_SPEC contract from the `resolve-skills` repo, consumable by Claude Code / Codex with zero changes.
- Search order: `$HARNESS_SKILLS_DIR` → `<cwd>/.resolve-tui-skills/` → built-in submodule.
- Adaptive activation: `triggers` match injects into context; no triggers means model self-selection.

## Multi-Agent (PSE) Mode

Tri-role orchestration inspired by `agentic-souls` (role definitions in `resolve-skills/souls/`):

- **Planner** (main agent): Task decomposition, only `delegate_specialist` + `evaluate` + read-only tools.
- **Specialist** (sub-loop): Execution (including writes), full tool set.
- **Evaluator** (sub-loop): Independent verification, read-only tools, outputs PASS/PARTIAL/FAIL/BLOCKED.

Enable: `--multi-agent` / `HARNESS_MULTI_AGENT=1` etc., or `/pse on|off` in TUI (takes effect next round).

## MCP Support

- Dynamically add MCP servers: `/mcp add <name> <command> [args...]`
- Remove MCP servers: `/mcp remove <name>`
- List configured MCP servers: `/mcp list`
- Supports stdio-transport MCP servers (filesystem, GitHub, databases, etc.)

Examples:
```bash
# Filesystem MCP
/mcp add fs npx -y @modelcontextprotocol/server-filesystem /path/to/dir

# GitHub MCP
/mcp add gh npx -y @modelcontextprotocol/server-github
```

> Note: MCP server `env` configuration is stored in plaintext in `config.toml`, be mindful of file permissions.

## Sandbox

- Default root: `<project>/.resolve-tui-sandbox/` (override with `HARNESS_SANDBOX_DIR`), created on startup.
- **Per-task independent workspace** `<root>/task-<nanos>-<pid>/`: `write_file`/shell relative paths and artifacts all land
  in this workspace, tasks don't overwrite each other; workdir and write-whitelist is the workspace.
- **Read scope**: project dir + workspace + system temp dir (`read_file`/`list_dir`), files outside are rejected—
  prevents local sensitive files (`~/.ssh`, etc.) from being read into context and sent out.
- Shell isolated via macOS `sandbox-exec` / Linux `bwrap`: network disabled by default, writes limited to whitelist.
- Auto-cleans workspaces older than 7 days on startup (under `HARNESS_SANDBOX_DIR` root).
- System prompt injects "current writable workspace + whitelist + read scope", model no longer guesses paths.

## Sessions & Memory

- Auto-save on exit, auto-resume on start; manage with `/list /create /apply /save /load /clear /rm`.
- Session dir: `.resolve-tui-sessions/` (gitignored, saved with 0600 permissions).
- `/remember` for cross-session long-term memory, `MEMORY.md` stored in system config dir (0600).

## Environment Variables

| Variable | Description | Default |
|---|---|---|
| `OPENAI_API_KEY` | API key (can use macOS Keychain) | — |
| `OPENAI_API_BASE` | OpenAI-compatible base (without `/responses`) | `https://api.openai.com/v1` |
| `HARNESS_MODEL` | Main model | `gpt-4o-mini` |
| `HARNESS_CODEGEN` / `_MODEL` / `_DIR` | codegen switch / cheap model / plugin dir | on / follows main model |
| `HARNESS_SANDBOX` / `_ALLOW_NETWORK` / `_ROOTS` / `_DIR` | sandbox switch / network / write whitelist / root dir | on / off / cwd+temp |
| `HARNESS_MULTI_AGENT` | Default PSE multi-agent | off |
| `HARNESS_MAX_ITERATIONS` / `_MAX_TOKENS` / `_HISTORY_MAX_ITEMS` | iterations / token budget / history window | 16 / unlimited / 200 |
| `HARNESS_SKILLS_DIR` / `PSE_SOULS_DIR` | skills / PSE tri-role souls dir | `./resolve-skills/skills` / `./resolve-skills/souls` |
| `HARNESS_SESSIONS_DIR` | sessions dir | `.resolve-tui-sessions` |
| `HARNESS_STATEFUL` / `_FORCE_TOOLS` / `_APPROVE_TOOLS` / `_THEME` | stateful resume / force tools / approval / theme | off |

## TUI Commands

`/pse [on|off]` · `/sandbox [clean]` · `/model [name]` · `/skills [reload]` · `/mcp add|remove|list`
· `/tools [on|off name]` · `/remember` · `/list /create /apply /save /load /clear /rm` · `/export`
· `/reasoning` · `/examples` · `/help`

## Build & Test

```bash
# Build
cargo build

# Run tests
cargo test

# Format code
cargo fmt

# Lint
cargo clippy
```

## Documentation

- [`docs/architecture.md`](./docs/architecture.md) — Architecture design doc (three-tier chain / PSE tri-role / sandbox / MCP)
- [`docs/examples.md`](./docs/examples.md) — Usage examples and FAQ
- [`docs/mcp.md`](./docs/mcp.md) — MCP server configuration and usage
- [`docs/audit-report.md`](./docs/audit-report.md) — Security audit report

## Privacy Posture

- No telemetry collected; no data sent to any third party except the chosen LLM API.
- Keys only stored in environment variables / system keychain, never in URLs, logs, or commits (`.env` is gitignored).
- Sessions, memory, sandbox workspaces are not committed; sensitive files saved with 0600 permissions.
- Boundary note: task content, injected `AGENT.md`/`MEMORY.md`, tool parameters and command outputs are sent with LLM requests—
  don't feed top-secret content to the model.

## Project Structure

```
resolve-tui/
├── resolve-skills/     # git submodule: skills pack + PSE tri-role souls
│   ├── skills/         #   code-review / post-comment / rust-review / weekly-investment-review
│   └── souls/          #   planner / specialist / evaluator
├── src/
│   ├── agent/          # PSE orchestration (roles) + main loop (drive/helpers)
│   ├── codegen/        # codegen plugins + isolated detector sandbox
│   ├── config/         # config (TOML + env table-driven merge)
│   ├── fastpath/       # deterministic fastpath
│   ├── llm.rs          # Responses API client (streaming/retry/usage)
│   ├── memory.rs       # cross-session long-term memory
│   ├── sandbox.rs      # sandbox policy / task workspace / seatbelt·bwrap
│   ├── sessions.rs     # session directory management
│   ├── skills.rs       # SKILL_SPEC skill loader (incl. souls)
│   └── tui/            # interactive UI (App / render / commands)
├── docs/               # docs (architecture / examples / mcp / audit-report)
├── examples/           # code examples
├── tests/              # integration tests
├── .env.example        # env var template
├── .gitmodules         # submodule config
└── Cargo.toml
```

## Related Articles
- [From Chat UI to Agent Workbench: How the Interaction Layer of a Terminal Coding Agent Upgraded](https://erishen.cn/resolve_tui-en/)

## License

MIT
