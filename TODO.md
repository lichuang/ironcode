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

1. **Persistent Sessions & Context** - Complete session persistence
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
12. **Approval System** - Rich text diff display
13. **Print/Non-interactive Mode** - `--print` mode
14. ~~**Auto-retry with Backoff**~~ [x] COMPLETED - Exponential backoff retry
15. ~~**YOLO Mode**~~ [x] COMPLETED - Auto-approve all operations with session-level persistence
16. **Background Task & Notification System** - Async workers with heartbeat and LLM context delivery
17. **Hook Engine** - Extensible PreToolUse/PostToolUse/UserPromptSubmit hooks
18. **Plan Mode** - Structured planning with EnterPlanMode/ExitPlanMode tools
19. **Git Context Integration** - Auto-inject git status/diff into system prompt
20. **Think Tool** - Explicit reasoning tool for complex problem-solving
21. **Wire Protocol Foundation** - Internal IPC for UI decoupling and multi-agent
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
- [ ] Background task creation and dispatch (`TaskList`, `TaskOutput`, `TaskStop` tools)
- [ ] Worker process with heartbeat and timeout monitoring
- [ ] Task output storage and retrieval
- [ ] Notification store and LLM context delivery

#### 17. Hook Engine
- [ ] Hook definition and registration framework
- [ ] PreToolUse / PostToolUse hooks
- [ ] UserPromptSubmit hook
- [ ] Stop hook for graceful interruption

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
- [ ] Auto-detect git repository
- [ ] Inject git status/diff summary into system prompt
- [ ] Git context for explore/codebase analysis

---

### Phase 3: Multi-Agent and Advanced Features
**Goal:** Complex task processing capabilities

#### 9. Subagent / Multi-Agent System
- [ ] Task tool design (create subtasks)
- [ ] Agent pool management (LaborMarket pattern)
- [ ] Parent-child session isolation and communication
- [ ] Sub-agent result aggregation
- [ ] Concurrent task execution limits

#### 10. Checkpoint / D-Mail System
- [ ] Context snapshot saving mechanism
- [ ] Checkpoint list and naming
- [ ] Rollback to specific checkpoint
- [ ] Branch session creation
- [ ] Checkpoint visualization (timeline UI)

#### 11. Skill System Foundation
- [ ] Skill file format design (YAML/JSON)
- [ ] Skill loading and parsing
- [ ] Skill variable substitution
- [ ] Flow skills foundation (simple workflows)

#### 20. Think Tool
- [ ] `Think` tool handler for explicit reasoning
- [ ] Integration with thinking mode streaming display

#### 21. Wire Protocol Foundation
- [ ] Define internal message types (TurnBegin, StepBegin, ToolCall, ApprovalRequest, etc.)
- [ ] Message hub for decoupling UI from core session logic
- [ ] Foundation for multi-agent communication and ACP

---

### Phase 4: Multimodal and Ecosystem Integration
**Goal:** Diverse interactions and IDE integration

#### 12. Print / Non-Interactive Mode
- [ ] `--print` command line argument
- [ ] Single-shot query mode (similar to `kimi "query"`)
- [ ] Pipe input support (`cat file.rs | ironcode`)
- [ ] JSON output mode (for scripting)

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
- [ ] Code parsing tools (AST-aware)
- [ ] Git integration tools (diff, blame, log)
- [ ] LSP client integration (code completion, go-to-definition)

#### 22. Plugin System
- [ ] Plugin discovery and loading mechanism
- [ ] External tool registration API
- [ ] Plugin manifest format and security isolation

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
| **Total Tasks** | 101 |
| **Completed** | 24 |
| **Remaining** | 77 |
| **Progress** | 23.8% |

> **Note:** This summary must be updated whenever tasks are completed. After each batch of completions, recalculate the counts and percentage to keep the document accurate.

## Notes

- Each Phase can be developed independently and in parallel
- Recommended to start from Phase 1, build foundation before entering Phase 2
- Phase 3 and 4 are advanced features, adjust priority based on user feedback
- Maintain ironcode's simplicity, avoid feature bloat
