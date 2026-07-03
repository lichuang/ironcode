---
name: Agent
description: Start a subagent instance to work on a focused task.
---

## Parameters

```json
{
  "type": "object",
  "properties": {
    "description": {
      "type": "string",
      "description": "Short (3-5 word) description of the task"
    },
    "prompt": {
      "type": "string",
      "description": "The task for the subagent to perform"
    },
    "subagent_type": {
      "type": "string",
      "description": "Built-in agent type: coder, explore, or plan. Default: coder",
      "default": "coder"
    },
    "model": {
      "type": "string",
      "description": "Optional model alias override"
    },
    "resume": {
      "type": "string",
      "description": "Optional agent ID to resume an existing instance"
    },
    "run_in_background": {
      "type": "boolean",
      "description": "Run the subagent as a background task. Use TaskList/TaskOutput/TaskStop to monitor it.",
      "default": false
    },
    "timeout": {
      "type": "integer",
      "description": "Optional timeout in seconds (30-3600)"
    }
  },
  "required": ["description", "prompt"]
}
```

Start a subagent instance to work on a focused task.

**Available Built-in Agent Types**

- `coder`: Good at general software engineering tasks.
- `explore`: Fast codebase exploration with prompt-enforced read-only behavior.
- `plan`: Read-only implementation planning and architecture design.

**Usage**

- Always provide a short `description` (3-5 words).
- Use `subagent_type` to select a built-in agent type. If omitted, `coder` is used.
- Use `model` when you need to override the parent agent's current model.
- The subagent result is only visible to you. If the user should see it, summarize it yourself.

**Explore Agent — Preferred for Codebase Research**

When you need to understand the codebase before making changes, fixing bugs, or planning features,
prefer `subagent_type="explore"` over doing the search yourself. The explore agent is optimized for
fast, read-only codebase investigation. Use it when:
- Your task will clearly require more than 3 search queries
- You need to understand how a module, feature, or code path works
- You are about to enter plan mode and want to gather context first

When calling explore, specify the desired thoroughness in the prompt:
- "quick": targeted lookups — find a specific file, function, or config value
- "medium": understand a module — how does auth work, what calls this API
- "thorough": cross-cutting analysis — architecture overview, dependency mapping, multi-module investigation

**When Not To Use Agent**

- Reading a known file path
- Searching a small number of known files
- Tasks that can be completed in one or two direct tool calls
