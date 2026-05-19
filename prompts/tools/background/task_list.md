---
name: TaskList
description: List background tasks from the current session.
---

List background tasks from the current session.

Use this when you need to re-enumerate which background tasks still exist, especially after context compaction or when you are no longer confident which task IDs are still active.

Guidelines:

- Prefer the default `active_only=true` unless you specifically need completed or failed tasks.
- Use `TaskOutput` to inspect one task in detail after you have identified the correct task ID.
- Do not guess which tasks are still running when you can call this tool directly.
- This tool is read-only and safe to use in plan mode.

## Parameters

```json
{
  "type": "object",
  "properties": {
    "active_only": {
      "type": "boolean",
      "description": "Whether to list only non-terminal background tasks.",
      "default": true
    },
    "limit": {
      "type": "integer",
      "description": "Maximum number of tasks to return.",
      "default": 20,
      "minimum": 1,
      "maximum": 100
    }
  }
}
```
