# Hygiene

## When to use

Inspect or clean memory hygiene.

The default `minimal` preset does not list this group, so `tools/list` may not show it, and a tool a preset does not list stays callable by name.

## Tools

`hygiene_audit`, `hygiene_clean`, `hygiene_log_list`

## Examples

`hygiene_clean` without `confirm: true` is a dry run: it reports the counts it would act on and writes nothing.

```
hygiene_audit({"min_score": 0.4})
hygiene_clean({"item_ids": ["demo-note"], "action": "flag"})
```

## Exact arguments

Call `tool_describe` with a tool name from the list above for the exact arguments. For example: `tool_describe("hygiene_audit")`. `tool_describe` exists only when discovery is enabled (`--discovery on` or `mcp.discovery: true`).
