---
name: SetTodoList
description: Update the whole todo list. Use this tool when the given task involves multiple subtasks/milestones, or multiple tasks are given in a single request. This tool can help you break down the task and track the progress.
---

## Parameters

```json
{
  "type": "object",
  "properties": {
    "todos": {
      "type": "array",
      "description": "The updated todo list",
      "items": {
        "type": "object",
        "properties": {
          "title": {
            "type": "string",
            "description": "The title of the todo"
          },
          "status": {
            "type": "string",
            "description": "The status of the todo",
            "enum": ["pending", "in_progress", "done"]
          }
        },
        "required": ["title", "status"]
      }
    }
  },
  "required": ["todos"]
}
```
