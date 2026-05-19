# IronCode Development TODO

## Project Comparison: ironcode vs kimi-cli

### Summary

| Dimension | **kimi-cli** (Python) | **ironcode** (Rust) |
|-----------|----------------------|---------------------|
| **Architecture Maturity** | Enterprise-grade (multi-agent, MCP, ACP) | Lightweight (single session, basic tools) |
| **UI Modes** | Shell / Web / ACP / VS Code | TUI only |
| **Session Management** | Persistent sessions + checkpoints | Basic persistence layer ready |
| **Context Management** | Automatic compaction | Automatic compaction (rolling window) |
| **Multi-agent** | LaborMarket + Task tools | None |
| **MCP Support** | Full (management CLI + loading) | Framework exists, not implemented |
| **Authentication** | OAuth device flow | API key only |
| **Skill System** | Standard + Flow skills | None |
| **Web Search** | Dedicated Moonshot service | DuckDuckGo |

### Missing Features (kimi-cli has, ironcode lacks)

1. ~~**Persistent Sessions & Context**~~ [x] COMPLETED - JSONL session store + metadata + auto-save
2. ~~**Context Compaction**~~ [x] COMPLETED - Automatic context compression with rolling window strategy
3. **Subagent/Multi-agent System** - Task tool + LaborMarket
4. **MCP Full Support** - `kimi mcp` management CLI
5. **Web UI** - FastAPI + WebSocket backend
6. **Skill System** - Standard skills + Flow skills (Mermaid/D2)
7. **OAuth & Authentication** - Device flow OAuth
8. **Advanced Web Tools** - Dedicated search/scraping services
9. **Structured Ask User** - Structured question system
10. **D-Mail / Checkpoint** - Time travel rollback functionality
11. **ACP Protocol** - IDE integration protocol
12. ~~**Approval System**~~ [x] COMPLETED - Diff display for WriteFile/ReplaceFile, YOLO mode, per-tool auto-approve
13. **Print/Non-interactive Mode** - `--print` mode
14. ~~**Auto-retry with Backoff**~~ [x] COMPLETED - Exponential backoff retry
15. ~~**YOLO Mode**~~ [x] COMPLETED - Auto-approve all operations with session-level persistence
16. **Background Task & Notification System** - Async workers with heartbeat and LLM context delivery
17. **Hook Engine** - Extensible PreToolUse/PostToolUse/UserPromptSubmit hooks
18. **Plan Mode** - Structured planning with EnterPlanMode/ExitPlanMode tools
19. **Git Context Integration** - Auto-inject git status/diff into system prompt
20. **Think Tool** - Explicit reasoning tool for complex problem-solving
21. ~~**Wire Protocol Foundation**~~ [x] COMPLETED - WireBus (broadcast) + 19 WireMessage types decouple LLM from UI
22. **Plugin System** - Dynamic external tool loading
23. **Session Export** - Export conversations to markdown/JSON
24. **Auto-update** - Version check and self-update mechanism

---

## Development Plan (By Priority)

### Phase 1: Infrastructure Strengthening
**Goal:** Build scalable foundation, address core pain points

#### 1. Session Persistence System [x] COMPLETED
- [x] Design session storage format (JSON/JSONL)
- [x] Implement `~/.ironcode/sessions/` directory management
- [x] Session saving (auto/manual trigger) - store layer integrated with ChatSession Actor
- [x] Session metadata (title, timestamps, message count)
- [ ] Session list UI (load historical sessions) - moved to Phase 2

#### 2. Context Compaction [x] COMPLETED
- [x] Token counting estimation (tiktoken-rs or approximate algorithm)
- [x] Compaction trigger threshold configuration
- [x] Rolling window strategy (keep recent N messages)
- [x] Compaction event notification UI
- [x] Automatic compaction execution
- [ ] Summary generation strategy (LLM-generated historical summary) - Future enhancement

#### 3. Improved Error Handling and Retry [x] COMPLETED
- [x] Integrate custom exponential backoff retry logic
- [x] Precise error classification for retry decisions (mirrors kimi-cli)
- [x] LLM stream interruption recovery mechanism
- [x] User-friendly network error messages
- [x] Configurable retry count and delay

---

### Phase 2: Core Feature Enhancement
**Goal:** Reach productive tool level

#### 4. Session List UI
- [ ] TUI session list interface (browse historical sessions)
- [ ] Session search and filtering
- [ ] Session deletion functionality

#### 5. Enhanced Approval System
- [x] YOLO mode (`--yolo` / config option)
- [x] File modification diff display
- [x] Shell command preview confirmation
- [x] Per-tool auto-approval configuration (e.g., only auto-approve ReadFile)

#### 6. Structured AskUser Tool
- [x] Single-select/multi-select question types
- [x] Confirmation dialogs (yes/no)
- [x] Input validation and default values
- [ ] Rich text rendering (Markdown)

#### 7. Full MCP Support
- [x] MCP server configuration format design
- [ ] `ironcode mcp` subcommand framework
- [ ] `mcp add/remove/list` implementation
- [ ] MCP tool dynamic loading and registration
- [ ] MCP server lifecycle management

#### 8. OAuth Authentication Support
- [ ] Kimi OAuth device flow implementation
- [ ] Token refresh mechanism
- [ ] `ironcode login/logout` commands
- [ ] Secure token storage (keychain/keyring)

#### 16. Background Task & Notification System
- [x] Background task creation and dispatch (`TaskList`, `TaskOutput`, `TaskStop` tools)
- [x] Worker process with heartbeat and timeout monitoring
- [x] Task output storage and retrieval
- [x] TaskStop approval flow (kill should request user approval like kimi-cli)
- [x] `/task` slash command infrastructure (TUI slash commands for background tasks)
- [ ] Worker recovery on app restart (scan task dirs, reconnect to running workers)
- [ ] Notification store and LLM context delivery
  - Design `Notification` data model (category, severity, title, body, payload)
  - Notification persistence (JSONL or SQLite) per session
  - Polling mechanism in `SessionActor` or `App` to detect new terminal tasks
  - Injection strategy: prepend to user message or append to system prompt
  - Deduplication by `dedupe_key` (e.g., `background_task:{task_id}:{status}`)
  - Auto-clear notifications after they are consumed by LLM context

#### 17. Hook Engine
- [ ] Hook definition and registration framework
  - Load from config (TOML `[[hooks]]` entries with `event`, `command`, `script_path`)
  - In-process hook registry (`HookEngine`) attached to `Runtime`
- [ ] PreToolUse / PostToolUse hooks
  - Return `allow` (continue) / `block` (reject with message) / `modify` (mutate arguments)
  - Error handling: hook failure defaults to `allow` or `block` based on config
- [ ] UserPromptSubmit hook
  - Triggered after user input but before sending to LLM
  - Can mutate prompt text or inject context
- [ ] Stop hook for graceful interruption
  - Triggered on Ctrl+C or explicit stop
  - Async cleanup window (e.g., save state, flush logs)

#### 18. Plan Mode
- [x] `EnterPlanMode` / `ExitPlanMode` tools
- [x] Plan session isolation and state persistence
- [ ] Plan slug tracking in session state
- [x] EnterPlanMode handler blocking user confirmation (via SessionActor interception)
- [x] ExitPlanMode plan content reading from `~/.ironcode/plans/{session_id}.md`
- [x] ExitPlanMode plan approval UI — Approve / Reject / Reject and Exit (+ custom options)
- [x] PlanDisplay wire message for rendering plan content in TUI
- [ ] QuestionRequest "other" option support (Revise free-text input) — TUI does not support free-text answers yet
- [x] YOLO mode auto-approve logic for EnterPlanMode/ExitPlanMode
- [ ] `/plan` slash command (toggle/view/clear) — requires slash command infrastructure
- [x] Session-scoped plan file path injection into plan handlers

#### 19. Git Context Integration
- [ ] Auto-detect git repository (walk up from cwd to find `.git`)
- [ ] Inject git status/diff summary into system prompt
  - Limit diff length (e.g., max 200 lines) to avoid token bloat
  - Refresh strategy: on session start + after file-modifying tool executions
  - Format: markdown code block with `git status --short` + `git diff --cached --stat`
- [ ] Git context for explore/codebase analysis
  - `git log --oneline -n 20` for recent history
  - `git branch -a` for branch awareness

---

### Phase 3: Multi-Agent and Advanced Features
**Goal:** Complex task processing capabilities

#### 9. Subagent / Multi-Agent System
- [ ] Task tool design (create subtasks)
  - `Agent` tool parameters: `subagent_type`, `prompt`, `description`, `timeout`, `model_override`
- [ ] Agent pool management (LaborMarket pattern)
  - `LaborMarket` registry: builtin types + dynamically registered types
  - `AgentTypeDefinition`: name, system_prompt, tool_policy, max_steps
- [ ] Parent-child session isolation and communication
  - Child `SessionActor` runs in separate tokio task with own `Context`
  - Communication via `WireBus` (child publishes `SubagentEvent`, parent subscribes)
  - OR reuse existing `mpsc` channel if WireBus latency is unacceptable
- [ ] Sub-agent result aggregation
  - Child produces final `Message::assistant` → parent injects as `Message::system` or tool result
- [ ] Concurrent task execution limits
  - Configurable `max_concurrent_subagents` (default 3)

#### 10. Checkpoint / D-Mail System
- [ ] Context snapshot saving mechanism
  - Leverage existing `SessionStore` (JSONL copy) + explicit `checkpoint()` API on `Context`
  - Snapshot includes messages + session meta (yolo, plan_mode, title)
- [ ] Checkpoint list and naming
  - Auto-name from first user message after checkpoint, or allow explicit naming
  - Stored in `~/.ironcode/sessions/{session_id}/checkpoints/{checkpoint_id}.jsonl`
- [ ] Rollback to specific checkpoint
  - Replace current `Context` messages with checkpoint snapshot
  - **Caveat**: cannot undo file system side effects (document this limitation)
- [ ] Branch session creation
  - Copy checkpoint to new session directory, generate new session ID
- [ ] Checkpoint visualization (timeline UI)
  - Optional TUI panel showing checkpoint list with timestamps

#### 11. Skill System Foundation
- [ ] Skill file format design (YAML/JSON)
  - Standard skill: text template with `{{variable}}` placeholders
  - Flow skill: DAG of steps (Mermaid/D2 syntax for visualization)
- [ ] Skill loading and parsing
  - Discovery roots: builtin (`~/.ironcode/skills/`), project (`.kimi/skills/`), user
- [ ] Skill variable substitution
  - Scope: `RuntimeArgs` (now, work_dir, etc.) + user-provided args at invocation time
  - Substitution engine: simple string replacement or `tera`/`handlebars`
- [ ] Flow skills foundation (simple workflows)
  - Step types: `llm_call`, `tool_call`, `conditional`, `parallel`
  - Output of one step accessible as `{{steps.step_name.output}}`

#### 20. Think Tool
- [ ] `Think` tool handler for explicit reasoning
- [ ] Integration with thinking mode streaming display

#### 21. Wire Protocol Foundation [x] COMPLETED
- [x] Define internal message types (TurnBegin, StepBegin, ToolCall, ApprovalRequest, etc.)
  - Implemented: `WireMessage` enum with 19 variants in `src/wire/protocol.rs`
- [x] Message hub for decoupling UI from core session logic
  - Implemented: `WireBus` (`tokio::sync::broadcast`, capacity 4096) in `src/wire/bus.rs`
- [x] Foundation for multi-agent communication and ACP
  - `WirePublisher` / `WireSubscriber` pattern supports multiple consumers; ACP can reuse

---

### Phase 4: Multimodal and Ecosystem Integration
**Goal:** Diverse interactions and IDE integration

#### 12. Print / Non-Interactive Mode
- [ ] `--print` command line argument
  - Mutually exclusive with TUI; enters non-interactive mode immediately
- [ ] Single-shot query mode (similar to `kimi "query"`)
  - CLI positional argument: `ironcode "explain this code"`
  - Execute one turn (including tool loops) then exit
- [ ] Pipe input support (`cat file.rs | ironcode`)
  - Detect `stdin` is not a TTY, read as user message content
  - If `--print` is also set, treat piped content as the query
- [ ] JSON output mode (for scripting)
  - `--format json` output: `{ "messages": [...], "tool_calls": [...], "exit_code": 0 }`
  - Tool approval policy in print mode: requires `--yolo` or pre-configured auto-approve

#### 13. Web UI (Optional)
- [ ] FastAPI + WebSocket backend design
- [ ] Frontend framework selection (React/Vue/Svelte)
- [ ] Share core logic with TUI (Session, Tool)
- [ ] File upload support

#### 14. ACP Protocol Support (Optional)
- [ ] Agent Client Protocol specification research
- [ ] ACP server implementation
- [ ] VS Code extension compatibility testing

#### 15. Enhanced Tool Set
- [ ] Image/vision support (image input)
  - Depends on model capability (`supports_vision` in `ModelConfig`)
  - TUI: image path paste or drag-and-drop detection
- [ ] Advanced Web Tools (dedicated search/scraping services)
  - Moonshot-style dedicated search endpoint (if API available)
  - Structured scraping (JSON-LD, meta tags) beyond raw HTML
- [ ] Code parsing tools (AST-aware)
  - Tree-sitter integration for language-aware symbol extraction
- [ ] Git integration tools (diff, blame, log)
  - Reuse `src/tools/handlers/shell/bash.rs` initially; migrate to native git2 later
- [ ] LSP client integration (code completion, go-to-definition)
  - Long-term: spawn LSP servers, expose as tools (optional)

#### 22. Plugin System
- [ ] Plugin discovery and loading mechanism
  - Search paths: `~/.ironcode/plugins/` and project-local `.ironcode/plugins/`
  - Load at startup (before `Runtime::new()`) or hot-reload via `/reload`
- [ ] External tool registration API
  - Plugin defines tools via JSON Schema + WASM function or executable script
  - Register into `ExecutableToolRegistry` alongside built-in handlers
- [ ] Plugin manifest format and security isolation
  - Manifest: name, version, permissions (fs-read, fs-write, network, shell)
  - Isolation: WASM sandbox or subprocess with restricted env

#### 23. Session Export
- [ ] Export conversation to markdown/plaintext
- [ ] Export to JSON/JSONL format
- [ ] `ironcode export` CLI command

#### 24. Auto-update
- [ ] Version check against GitHub releases
- [ ] Self-update mechanism
- [ ] Update notification in TUI startup


---

## Completed Features (Archived)

### [x] Session Persistence System (Phase 1)
- Storage layer (JSONL + `SessionStore`)
- Integration with `ChatSession` Actor
- `--session <ID>` and `--continue` argument support
- Automatic message and metadata saving

---

## Summary

| Metric | Count |
|--------|-------|
| **Total Tasks** | 105 |
| **Completed** | 38 |
| **Remaining** | 67 |
| **Progress** | 36.2% |

> **Note:** This summary must be updated whenever tasks are completed. After each batch of completions, recalculate the counts and percentage to keep the document accurate.

## Notes

- Each Phase can be developed independently and in parallel
- Recommended to start from Phase 1, build foundation before entering Phase 2
- Phase 3 and 4 are advanced features, adjust priority based on user feedback
- Maintain ironcode's simplicity, avoid feature bloat
