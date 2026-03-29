---
name: ReplaceFile
description: Replace specific strings within a specified file. Use this when you need to make targeted edits to an existing file.
---

## Parameters

```json
{
  "type": "object",
  "properties": {
    "path": {
      "type": "string",
      "description": "The path to the file to edit. Absolute paths are required when editing files outside the working directory."
    },
    "edit": {
      "oneOf": [
        {
          "type": "object",
          "properties": {
            "old": {
              "type": "string",
              "description": "The old string to replace. Can be multi-line."
            },
            "new": {
              "type": "string",
              "description": "The new string to replace with. Can be multi-line."
            },
            "replace_all": {
              "type": "boolean",
              "description": "Whether to replace all occurrences.",
              "default": false
            }
          },
          "required": ["old", "new"]
        },
        {
          "type": "array",
          "items": {
            "type": "object",
            "properties": {
              "old": {
                "type": "string",
                "description": "The old string to replace. Can be multi-line."
              },
              "new": {
                "type": "string",
                "description": "The new string to replace with. Can be multi-line."
              },
              "replace_all": {
                "type": "boolean",
                "description": "Whether to replace all occurrences.",
                "default": false
              }
            },
            "required": ["old", "new"]
          }
        }
      ],
      "description": "The edit(s) to apply to the file. You can provide a single edit or a list of edits here."
    }
  },
  "required": ["path", "edit"]
}
```

**Tips:**
- Only use this tool on text files.
- Multi-line strings are supported.
- Can specify a single edit or a list of edits in one call.
- You should prefer this tool over WriteFile tool and Shell `sed` command.
