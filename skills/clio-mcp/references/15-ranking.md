# Ranking

## When to use

Read or change ranking settings.

Only the `all` preset lists this group, so `tools/list` may not show it under another preset, and a tool a preset does not list stays callable by name.

## Tools

`ranking_env_get`, `ranking_env_set`

## Examples

`ranking_env_set` writes nothing when `dry_run` is true.

```
ranking_env_get({})
ranking_env_set({"patch": {"retention": {"half_life_days": 45}}, "dry_run": true})
```

## Exact arguments

Call `tool_describe` with a tool name from the list above for the exact arguments. For example: `tool_describe("ranking_env_get")`. `tool_describe` exists only when discovery is enabled (`--discovery on` or `mcp.discovery: true`).
