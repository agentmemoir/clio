# Retention

## When to use

Read or set retention policy.

The `ops` and `all` presets list this group, while the `minimal` and `coding` presets do not; a tool a preset does not list stays callable by name, because the call path gates on bound tools, not on the listing.

## Tools

`retention_profile_get`, `retention_profile_set`

## Examples

`retention_profile_set` writes nothing unless `confirm` is true, so a call without it is a preview.

```
retention_profile_get({"bank": "demo-bank"})
retention_profile_set({"profile": {}, "confirm": true})
```

## Exact arguments

Call `tool_describe` with a tool name from the list above for the exact arguments. For example: `tool_describe("retention_profile_get")`. `tool_describe` exists only when discovery is enabled (`--discovery on` or `mcp.discovery: true`).
