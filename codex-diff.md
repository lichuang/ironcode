# Codex (OpenAI) vs Kimi-CLI (Moonshot) 架构差异对比

## 1. 技术栈差异

| 方面 | Codex (OpenAI) | Kimi-CLI (Moonshot) |
|------|----------------|---------------------|
| **主要语言** | Rust + TypeScript | Python |
| **项目结构** | 多 crate Workspace (66+ crates) | Python 包结构 |
| **TUI 框架** | ratatui (Rust) | 自定义 Shell/Print/ACP 多前端 |
| **配置格式** | TOML | TOML + JSON |

---

## 2. 整体架构差异

### Codex
```
codex-rs/
├── core/              # 核心逻辑 crate
├── tui/               # 终端 UI
├── cli/               # 命令行接口
├── protocol/          # 协议类型定义
└── config/            # 配置管理
```

### Kimi-CLI
```
src/kimi_cli/
├── soul/              # 核心 Agent 运行时
├── wire/              # 通信协议层（Soul/UI 解耦）
├── ui/                # UI 前端（多前端支持）
├── tools/             # 工具实现
├── agents/            # Agent 规范定义
└── skill/             # 内置技能
```

**核心差异**：
- **Codex** 采用传统的分层 crate 架构，按功能模块划分
- **Kimi-CLI** 采用 **Soul/Wire/UI 三层解耦架构**，通过 Wire 协议通信，支持多种前端（Shell TUI、Print、ACP、Web）

---

## 3. 工具系统实现差异

### 3.1 核心 trait/基类

**Codex (Rust)**
```rust
#[async_trait]
pub trait ToolHandler: Send + Sync {
    fn kind(&self) -> ToolKind;
    async fn is_mutating(&self, _invocation: &ToolInvocation) -> bool;
    async fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, FunctionCallError>;
}

pub struct ToolRegistry {
    handlers: HashMap<String, Arc<dyn ToolHandler>>,
}
```

**Kimi-CLI (Python)**
```python
class CallableTool2(Generic[ParamsT]):
    name: str
    params: type[ParamsT]
    
    async def __call__(self, params: ParamsT) -> ToolReturnValue:
        ...

class KimiToolset:
    # 支持依赖注入
    def load_tools(self, tool_paths: list[str], dependencies: dict[type[Any], Any]):
        ...
```

### 3.2 工具参数定义

**Codex**
- 使用 JSON Schema 定义参数（Markdown 文件）
- 手动解析和验证

**Kimi-CLI**
- 使用 **Pydantic 模型**定义参数
- 自动生成 JSON Schema
- 类型安全，支持复杂验证

### 3.3 工具调用上下文

**Codex**
```rust
pub struct ToolInvocation {
    pub tool_name: Arc<str>,
    pub call_id: String,
    pub payload: ToolPayload,      // Function | Mcp
    pub cwd: PathBuf,
    pub session: Arc<Session>,
    pub turn: Arc<TurnContext>,
}
```

**Kimi-CLI**
- 通过依赖注入传递上下文（cwd、approval、wire 等）
- 更简洁，但需要工具显式声明依赖

---

## 4. Agent 架构差异

### 4.1 核心循环

**Codex**
```rust
// AgentControl - 弱引用模式避免循环
pub(crate) struct AgentControl {
    manager: Weak<ThreadManagerState>,
    state: Arc<Guards>,  // 限流控制
}
```
- 使用 **弱引用模式** 避免循环引用
- **Guard 限流**：限制最大线程数和深度
- Agent 状态机：Idle → Working → Executing → Finished/Error

**Kimi-CLI**
```python
class KimiSoul:
    async def _agent_loop(self) -> TurnOutcome:
        while True:
            step_no += 1
            # 自动 compact 上下文
            step_outcome = await self._step()
            # 处理 D-Mail、steer 等
```
- **Ralph Loop**: 自动迭代直到任务完成
- **D-Mail 系统**: "时间旅行"回退到 checkpoint
- **上下文自动压缩**

### 4.2 多 Agent 协调

**Codex**
- `multi_agents.rs` (75KB) - 复杂的多 Agent 协调
- `agent_jobs.rs` (43KB) - Agent 作业管理
- Agent 命名：从 `agent_names.txt` 分配昵称

**Kimi-CLI**
```python
class LaborMarket:
    fixed_subagents: dict[str, Agent]      # 预定义
    dynamic_subagents: dict[str, Agent]    # 动态创建（共享 LaborMarket）
```
- **劳动力市场**概念：子 Agent 可被动态创建和发现
- 动态子 Agent 共享 LaborMarket（可互相发现）

---

## 5. LLM 交互方式差异

### 5.1 客户端架构

**Codex**
```
ModelClient (Session-scoped)
    ↓
ModelClientSession (Turn-scoped)
    ↓
WebSocket / HTTP SSE
    ↓
OpenAI Responses API
```
- **Session 级别**：保存认证、会话 ID、提供商配置
- **Turn 级别**：每轮创建，缓存 WebSocket 连接
- **预连接优化**：WebSocket prewarm 减少首请求延迟
- **粘性路由**：`x-codex-turn-state` Header 保持路由一致性

**Kimi-CLI**
```python
# Kosong 框架
result = await kosong.step(
    chat_provider,
    system_prompt,
    toolset,
    context.history,
    on_message_part=wire_send,    # 流式回调
    on_tool_result=wire_send,
)
```
- 使用 **Kosong** 内部 LLM 抽象框架
- 支持多种 Provider：kimi、openai_legacy、openai_responses、anthropic、gemini
- 通过回调函数实现流式传输

### 5.2 协议/通信模式

**Codex: SQ/EQ 模式**
```rust
// Submission Queue / Event Queue
pub struct Submission { pub id: String, pub op: Op }
pub struct Event { pub id: String, pub msg: EventMsg }

pub enum Op {
    UserTurn { ... },
    Interrupt,
    RealtimeConversationStart,
}
```

**Kimi-CLI: Wire 协议**
```python
# BroadcastQueue - 支持多 UI 订阅
class BroadcastQueue(Generic[T]):
    # SPMC 通道：多订阅者

# 消息类型
TurnBegin / TurnEnd
StepBegin / StepEnd
ContentPart
ToolCall / ToolResult
ApprovalRequest / ApprovalResponse
```
- **广播队列**：支持多 UI 订阅
- **消息合并**：`MergeableMixin` 合并连续消息块
- **持久化**：`WireFile` 自动记录完整对话

---

## 6. 配置系统差异

### 6.1 配置层次

**Codex**
```
优先级（从高到低）：
1. CLI 覆盖参数
2. .env 环境变量
3. 项目级 codex/config.toml
4. 用户级 ~/.codex/config.toml
5. 系统默认值
```

**约束系统**：
```rust
pub struct Constrained<T> {
    value: T,
    source: ConstraintSource,  // 值来源追踪
}
```

**Kimi-CLI**
```
主配置: ~/.kimi/config.toml
MCP 配置: ~/.kimi/mcp.json
会话状态: ~/.kimi/sessions/
日志: ~/.kimi/logs/kimi.log
```

使用 **Pydantic 模型**：
```python
class Config(BaseModel):
    default_model: str
    default_thinking: bool
    default_yolo: bool
    models: dict[str, LLMModel]
    providers: dict[str, LLMProvider]
    loop_control: LoopControl
    services: Services
    mcp: MCPConfig
```

---

## 7. 安全与沙箱差异

### 7.1 沙箱系统

**Codex**
| 平台 | 沙箱类型 |
|------|----------|
| macOS | Seatbelt (`.sbpl` 配置文件) |
| Linux | Seccomp + Landlock |
| Windows | Restricted Token |

```rust
// 多层沙箱架构
工具调用 → 执行策略检查 → 用户确认? → SandboxManager.transform() → 平台特定执行
```

**Kimi-CLI**
- **无系统级沙箱**
- **审批系统**代替沙箱：
  ```python
  class Approval:
      _state: ApprovalState  # yolo + auto_approve_actions
      async def request(...) -> bool:
          # YOLO 模式直接通过
          # 否则发送 ApprovalRequest 到 Wire
  ```

---

## 8. MCP (Model Context Protocol) 差异

**Codex**
```rust
pub struct McpConnectionManager {
    clients: HashMap<String, RmcpClient>,
    tool_cache: HashMap<String, ToolInfo>,
}
// 工具名称格式: "mcp__{server}__{tool}"
const MCP_TOOL_NAME_DELIMITER: &str = "__";
```
- OAuth 认证流支持
- 工具名称规范化（Responses API 限制）

**Kimi-CLI**
```python
async def load_mcp_tools(self, mcp_configs: list[MCPConfig], ...):
    # 异步加载 MCP 工具
    # 支持后台连接
```
- 使用 **fastmcp** 集成
- 异步加载，支持后台连接

---

## 9. 独特功能对比

### Codex 独有
| 功能 | 说明 |
|------|------|
| **Seatbelt/Seccomp 沙箱** | 系统级安全隔离 |
| **Hooks 系统** | 工具调用后和 Agent 操作后的钩子 |
| **SQLite 状态存储** | 持久化会话历史 |
| **文件监听** | 配置变更自动重载 |
| **Event Sourcing** | Rollout 记录完整对话历史 |

### Kimi-CLI 独有
| 功能 | 说明 |
|------|------|
| **D-Mail 系统** | Checkpoint 回滚，"时间旅行"消息 |
| **Flow 技能** | Mermaid/D2 流程图定义 Agent 工作流 |
| **Agent 规范继承** | YAML 定义，支持 `extend: default` |
| **Skill 发现层级** | 项目级 > 用户级 > 内置 |
| **多前端支持** | Shell TUI、Print、ACP、Web |
| **Ralph Loop** | 自动迭代直到完成 |
| **上下文自动压缩** | 智能保留关键信息 |

---

## 10. 设计模式对比

| 模式 | Codex | Kimi-CLI |
|------|-------|----------|
| **工具处理** | Handler Pattern | Pydantic Callable |
| **注册发现** | Registry Pattern | 依赖注入 + 动态加载 |
| **生命周期** | Weak Reference | Context Manager |
| **通信** | SQ/EQ 队列 | Wire SPMC 广播 |
| **配置** | Layered + Constrained | Pydantic Model |
| **安全** | Capability-based (沙箱) | Approval-based (审批) |
| **前端** | 单一 TUI | 多前端架构 |

---

## 11. 代码规模对比

| 指标 | Codex | Kimi-CLI |
|------|-------|----------|
| 核心代码量 | codex.rs ~366KB | kimisoul.py ~中等规模 |
| TUI 代码量 | app.rs ~323KB | 分散在多前端 |
| 工具处理器 | 13+ 个，总计 ~300KB+ | 15+ 个，Python 实现 |
| Agent 系统 | 多线程复杂协调 | LaborMarket + D-Mail |

---

## 12. 总结

### Codex 特点
- **生产级安全**：多层沙箱、OAuth、SQLite 持久化
- **企业级架构**：严格的类型安全、弱引用避免循环、配置溯源
- **性能优化**：WebSocket 预连接、粘性路由
- **单一 TUI**：ratatui 深度定制

### Kimi-CLI 特点
- **灵活架构**：Soul/Wire/UI 解耦，多前端支持
- **创新功能**：D-Mail、Flow 技能、Agent 继承
- **Python 生态**：Pydantic、fastmcp、Kosong 框架
- **用户体验**：Ralph Loop、自动压缩、审批系统

### 关键架构决策差异
1. **安全模型**：Codex 采用系统级沙箱，Kimi-CLI 采用审批系统
2. **通信方式**：Codex 使用 SQ/EQ 队列，Kimi-CLI 使用 Wire SPMC 广播
3. **多 Agent**：Codex 是线程级多 Agent，Kimi-CLI 是劳动力市场模型
4. **前端架构**：Codex 单一 TUI，Kimi-CLI 多前端支持
5. **配置系统**：Codex 分层约束，Kimi-CLI Pydantic 模型
