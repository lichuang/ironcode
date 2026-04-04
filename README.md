# IronCode

AI-powered terminal code assistant built with Rust.

## Features

### Core Experience
- ✅ **TUI Interface** — Built with `ratatui` for an interactive terminal experience
- ✅ **Streaming Responses** — Real-time LLM response streaming with thinking/reasoning support
- ✅ **Input History** — Shell-like Up/Down navigation with persistent `history.jsonl`
- ✅ **Configuration via TOML** — Customizable models, providers, and settings in `~/.ironcode/config.toml`

### LLM & Providers
- ✅ **Kimi Provider** — Native Kimi API support with Coding Agent authentication headers
- ✅ **Thinking Mode** — Supports `<think>` reasoning content extraction
- ✅ **OpenAI-Compatible Framework** — Extensible provider trait for adding new models
- ❌ **Context Compaction** — Automatic token limiting and summary generation
- ❌ **Auto-Retry with Backoff** — Exponential backoff for failed requests

### Tool System
- ✅ **ReadFile** — Read file contents with line numbers and offset/limit
- ✅ **WriteFile** — Write or append content to files
- ✅ **ReplaceFile** — String replacement within files (single or multiple edits)
- ✅ **Glob** — Find files matching glob patterns
- ✅ **Grep** — Search file contents with ripgrep integration
- ✅ **Bash / PowerShell** — Execute shell commands with timeout support
- ✅ **SearchWeb** — Web search via DuckDuckGo
- ✅ **FetchURL** — Fetch and extract article content from URLs
- ✅ **SetTodoList** — Track task progress visually
- ✅ **AskUserQuestion** — Structured question definitions (TUI integration pending)
- ❌ **MCP Support** — Model Context Protocol server integration
- ❌ **Git Tools** — Diff, blame, log integration
- ❌ **LSP Integration** — Code completion and go-to-definition
- ❌ **AST-Aware Tools** — Code parsing and semantic analysis

### Sessions & Persistence
- ✅ **Session Persistence** — JSONL-based storage (`meta.json` + `context.jsonl`)
- ✅ **Resume by ID** — `ironcode --session <ID>` to resume a specific session
- ✅ **Resume Latest** — `ironcode --continue` to continue the most recent session
- ✅ **Auto-Save** — Automatic persistence on messages, tool results, and history clears
- ✅ **Session Metadata** — Title, timestamps, and message tracking
- ❌ **Session List UI** — Browse and load historical sessions from within the TUI
- ❌ **Checkpoints / D-Mail** — Snapshot and rollback to previous conversation states

### CLI & Modes
- ✅ **Custom Config Directory** — `-c / --config` to specify config location
- ❌ **Print / Non-Interactive Mode** — `--print` for single-shot queries and piping
- ❌ **YOLO Mode** — Auto-approve all tool executions
- ❌ **Approval System** — Diff previews and per-tool auto-approval settings

### Advanced Features
- ❌ **Multi-Agent / Subagent System** — Task delegation and LaborMarket-style agent pools
- ❌ **Skill System** — Reusable skills and flow-based workflows
- ❌ **Web UI** — FastAPI + WebSocket alternative interface
- ❌ **ACP Protocol** — Agent Client Protocol for IDE integration
- ❌ **OAuth Authentication** — Device flow token management
- ❌ **Vision / Image Input** — Multimodal message support

## Quick Start

```bash
# Run with a new session (default)
ironcode

# Continue the most recent session
ironcode --continue

# Resume a specific session
ironcode --session <SESSION_ID>

# Use a custom config directory
ironcode -c /path/to/config
```

## Configuration

Place your `config.toml` in `~/.ironcode/`:

```toml
default_model = "kimi/kimi-for-coding"

[providers.kimi]
type = "kimi"
base_url = "https://api.moonshot.cn/v1"
api_key = "${KIMI_API_KEY}"

[models."kimi/kimi-for-coding"]
provider = "kimi"
model = "kimi-for-coding"
max_context_size = 128000
supports_streaming = true
```
