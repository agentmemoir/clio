# Portability

## When to use

Move or review memory exports and imports.

## Tools

`export`, `import`, `import_provider`, `review_export`

## Examples

The export destination must lie inside the export root, and `import` reads a bundle a prior export wrote.

```
export({"destination": "exports/demo-bundle.json"})
import({"source": "exports/demo-bundle.json", "dry_run": true})
```

## Exact arguments

Call `tool_describe` with a tool name from the list above for the exact arguments. For example: `tool_describe("export")`.
