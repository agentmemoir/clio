# Hygiene

## When to use

Inspect or clean memory hygiene.

## Tools

`hygiene_audit`, `hygiene_clean`, `hygiene_log_list`

## Examples

```
hygiene_audit({"min_score": 0.4})
hygiene_clean({"item_ids": ["demo-note"], "action": "flag"})
```

## Exact arguments

Call `tool_describe` with a tool name from the list above for the exact arguments. For example: `tool_describe("hygiene_audit")`.
