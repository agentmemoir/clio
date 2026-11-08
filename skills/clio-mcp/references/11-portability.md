# Portability

## When to use

Move or review memory exports and imports.

The default `minimal` preset does not list this group, so `tools/list` may not show it, and a tool a preset does not list stays callable by name.

## Tools

`export`, `import`, `import_provider`, `review_export`

## Examples

`export` writes `destination` inside the export root and is refused unless the call acts as a `harness` or `system` actor, which the transport resolves from the session credential, the `actor` runtime argument or the session default, while `import` reads `source` from the server process working directory.

```
export({"destination": "demo-bundle.json"})
import({"source": "demo-bundle.json", "dry_run": true})
```

## Exact arguments

Call `tool_describe` with a tool name from the list above for the exact arguments. For example: `tool_describe("export")`. `tool_describe` exists only when discovery is enabled (`--discovery on` or `mcp.discovery: true`).
