---
name: WriteFile
description: Write content to a file at the specified path. Use this when you need to create a new file or overwrite an existing file.
---

## Parameters

```json
{
  "type": "object",
  "properties": {
    "path": {
      "type": "string",
      "description": "The path to the file to write. Absolute paths are required when writing files outside the working directory."
    },
    "content": {
      "type": "string",
      "description": "The content to write to the file"
    },
    "mode": {
      "type": "string",
      "description": "The mode to use to write to the file. Two modes are supported: 'overwrite' for overwriting the whole file and 'append' for appending to the end of an existing file.",
      "enum": ["overwrite", "append"],
      "default": "overwrite"
    }
  },
  "required": ["path", "content"]
}
```

**Tips:**
- When `mode` is not specified, it defaults to `overwrite`. Always write with caution.
- When the content to write is too long (e.g. > 100 lines), use this tool multiple times instead of a single call. Use `overwrite` mode at the first time, then use `append` mode after the first write.
