---
name: ReadFile
description: Read the contents of a file at the specified path. Use this when you need to examine the contents of a file.
---

## Parameters

```json
{
  "type": "object",
  "properties": {
    "path": {
      "type": "string",
      "description": "The path to the file to read"
    },
    "offset": {
      "type": "integer",
      "description": "The line number to start reading from (1-indexed)",
      "default": 1
    },
    "limit": {
      "type": "integer",
      "description": "The maximum number of lines to read",
      "default": 100
    }
  },
  "required": ["path"]
}
```
