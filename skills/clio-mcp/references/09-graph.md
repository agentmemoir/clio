# Graph

## When to use

Read or add memory relationships.

The `minimal` and `all` presets list this group, while the `coding` and `ops` presets do not; a tool a preset does not list stays callable by name, because the call path gates on bound tools, not on the listing. `tool_describe` and `tool_invoke` exist only when discovery is enabled (`--discovery on` or `mcp.discovery: true`), and discovery is off by default.

## Tools

`graph_link`, `graph_query`

## Examples

Both ids must already exist in the bank, because `graph_link` refuses an id that is not stored.

```
graph_link({"source_id": "itm-a", "target_id": "itm-b", "relationship": "supports"})
graph_query({"seed_id": "itm-a"})
```

## Exact arguments

Call `tool_describe` with a tool name from the list above for the exact arguments. For example: `tool_describe("graph_link")`.
