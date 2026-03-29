---
name: SearchWeb
description: WebSearch tool allows you to search on the internet to get latest information, including news, documents, release notes, blog posts, papers, etc.
---

## Parameters

```json
{
  "type": "object",
  "properties": {
    "query": {
      "type": "string",
      "description": "The query text to search for."
    },
    "limit": {
      "type": "integer",
      "description": "The number of results to return. Typically you do not need to set this value. When the results do not contain what you need, you probably want to give a more concrete query.",
      "default": 5,
      "minimum": 1,
      "maximum": 20
    },
    "include_content": {
      "type": "boolean",
      "description": "Whether to include the content of the web pages in the results. It can consume a large amount of tokens when this is set to True. You should avoid enabling this when limit is set to a large value.",
      "default": false
    }
  },
  "required": ["query"]
}
```
