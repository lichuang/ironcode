---
name: ExitPlanMode
description: Submit a plan for user approval while in plan mode. Before calling, the plan must be written to the plan file. This tool reads the plan file content and presents it to the user for approval. The user can select an implementation path (exit plan mode and start execution), reject (stay in plan mode and wait for feedback), or provide revision comments.
no_handler: false
---

## Parameters

```json
{
  "type": "object",
  "properties": {
    "options": {
      "type": "array",
      "description": "When the plan contains multiple alternative approaches, list them here so the user can choose which one to execute. 2-3 options. Each option represents a distinct approach from the plan. Do not use 'Reject', 'Revise', 'Approve', or 'Reject and Exit' as option labels.",
      "items": {
        "type": "object",
        "properties": {
          "label": {
            "type": "string",
            "description": "Short name for this option (1-8 words). Append '(Recommended)' if you recommend this option."
          },
          "description": {
            "type": "string",
            "description": "Brief summary of this approach and its trade-offs."
          }
        },
        "required": ["label"]
      },
      "maxItems": 3
    }
  }
}
```

Use this tool when you are in plan mode and have finished writing your plan to the plan file and are ready for user approval.

## How This Tool Works
- You should have already written your plan to the plan file specified in the plan mode reminder.
- This tool does NOT take the plan content as a parameter — it reads the plan from the file you wrote.
- The user will see the contents of your plan file when they review it.

## When to Use
Only use this tool for tasks that require planning implementation steps. For research tasks (searching files, reading code, understanding the codebase), do NOT use this tool.

## Multiple Approaches
If your plan contains multiple alternative approaches:
- Pass them via the `options` parameter so the user can choose which approach to execute.
- Each option should have a concise label and a brief description of trade-offs.
- If you recommend one option, append "(Recommended)" to its label.
- The user will see all options alongside Reject and Revise choices.
- Provide 2-3 options at most (the system appends a "Reject" option automatically, so the total shown to the user is 3-4).
- Do NOT use "Reject", "Revise", or "Approve" as option labels — these are reserved by the system.

## Before Using
- If you have unresolved questions, use AskUserQuestion first.
- If you have multiple approaches and haven't narrowed down yet, consider using AskUserQuestion first to let the user choose, then write a plan for the chosen approach only.
- Once your plan is finalized, use THIS tool to request approval.
- Do NOT use AskUserQuestion to ask "Is this plan OK?" or "Should I proceed?" — that is exactly what ExitPlanMode does.
- If rejected, revise based on feedback and call ExitPlanMode again.
