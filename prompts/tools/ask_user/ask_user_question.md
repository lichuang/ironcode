---
name: AskUserQuestion
description: Use this tool when you need to ask the user questions with structured options during execution. This allows you to collect user preferences, resolve ambiguous instructions, or let the user decide between implementation approaches.
---

## Parameters

```json
{
  "type": "object",
  "properties": {
    "questions": {
      "type": "array",
      "description": "The questions to ask the user (1-4 questions).",
      "minItems": 1,
      "maxItems": 4,
      "items": {
        "type": "object",
        "properties": {
          "question": {
            "type": "string",
            "description": "A specific, actionable question. End with '?'."
          },
          "header": {
            "type": "string",
            "description": "Short category tag (max 12 chars, e.g. 'Auth', 'Style').",
            "default": ""
          },
          "options": {
            "type": "array",
            "description": "2-4 meaningful, distinct options. Do NOT include an 'Other' option — the system adds one automatically.",
            "minItems": 2,
            "maxItems": 4,
            "items": {
              "type": "object",
              "properties": {
                "label": {
                  "type": "string",
                  "description": "Concise display text (1-5 words). If recommended, append '(Recommended)'."
                },
                "description": {
                  "type": "string",
                  "description": "Brief explanation of trade-offs or implications of choosing this option.",
                  "default": ""
                }
              },
              "required": ["label"]
            }
          },
          "multi_select": {
            "type": "boolean",
            "description": "Whether the user can select multiple options.",
            "default": false
          }
        },
        "required": ["question", "options"]
      }
    }
  },
  "required": ["questions"]
}
```

**When NOT to use:**
- When you can infer the answer from context — be decisive and proceed
- Trivial decisions that don't materially affect the outcome

Overusing this tool interrupts the user's flow. Only use it when the user's input genuinely changes your next action.

**Usage notes:**
- Users always have an "Other" option for custom input — don't create one yourself
- Use `multi_select` to allow multiple answers to be selected for a question
- Keep option labels concise (1-5 words), use descriptions for trade-offs and details
- Each question should have 2-4 meaningful, distinct options
- You can ask 1-4 questions at a time; group related questions to minimize interruptions
- If you recommend a specific option, list it first and append "(Recommended)" to its label
