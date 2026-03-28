---
name: Grep
description: A powerful search tool based-on ripgrep. Use this when you need to search for patterns in file contents. ALWAYS use Grep tool instead of running `grep` or `rg` command with Shell tool.
---

## Parameters

```json
{
  "type": "object",
  "properties": {
    "pattern": {
      "type": "string",
      "description": "The regular expression pattern to search for in file contents. Use the ripgrep pattern syntax, not grep syntax. E.g. you need to escape braces like \\{ to search for {."
    },
    "path": {
      "type": "string",
      "description": "File or directory to search in. Defaults to current working directory. If specified, it must be an absolute path.",
      "default": "."
    },
    "glob": {
      "type": "string",
      "description": "Glob pattern to filter files (e.g. '*.js', '*.{ts,tsx}'). No filter by default."
    },
    "output_mode": {
      "type": "string",
      "description": "'content': Show matching lines (supports -B, -A, -C, -n, head_limit); 'files_with_matches': Show file paths (supports head_limit); 'count_matches': Show total number of matches. Defaults to 'files_with_matches'.",
      "enum": ["content", "files_with_matches", "count_matches"],
      "default": "files_with_matches"
    },
    "before_context": {
      "type": "integer",
      "description": "Number of lines to show before each match (the -B option). Requires output_mode to be 'content'."
    },
    "after_context": {
      "type": "integer",
      "description": "Number of lines to show after each match (the -A option). Requires output_mode to be 'content'."
    },
    "context": {
      "type": "integer",
      "description": "Number of lines to show before and after each match (the -C option). Requires output_mode to be 'content'."
    },
    "line_number": {
      "type": "boolean",
      "description": "Show line numbers in output (the -n option). Requires output_mode to be 'content'.",
      "default": false
    },
    "ignore_case": {
      "type": "boolean",
      "description": "Case insensitive search (the -i option).",
      "default": false
    },
    "type": {
      "type": "string",
      "description": "File type to search. Examples: py, rust, js, ts, go, java, etc. More efficient than glob for standard file types."
    },
    "head_limit": {
      "type": "integer",
      "description": "Limit output to first N lines, equivalent to | head -N. Works across all output modes. By default, no limit is applied."
    },
    "multiline": {
      "type": "boolean",
      "description": "Enable multiline mode where . matches newlines and patterns can span lines (the -U and --multiline-dotall options). By default, multiline mode is disabled.",
      "default": false
    }
  },
  "required": ["pattern"]
}
```

**Tips:**
- ALWAYS use Grep tool instead of running `grep` or `rg` command with Shell tool.
- Use the ripgrep pattern syntax, not grep syntax. E.g. you need to escape braces like `\\{` to search for `{`.
