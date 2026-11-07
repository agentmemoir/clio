# Ops

## When to use

Diagnose, verify or repair operations.

The default `minimal` preset does not list this group, so `tools/list` may not show it, and a tool a preset does not list stays callable by name.

## Tools

`diagnose`, `verify`, `doctor`, `repair`, `reindex`, `stats`

## Examples

Start with the read-only checks; `doctor` prints a plan unless told otherwise, `repair` and `reindex` need `confirm` before they change anything, and `reindex` is refused when `bank` is omitted while the shared bank surface is disabled.

```
diagnose({})
verify({})
```

## Exact arguments

Call `tool_describe` with a tool name from the list above for the exact arguments. For example: `tool_describe("diagnose")`. `tool_describe` exists only when discovery is enabled (`--discovery on` or `mcp.discovery: true`).
