# Item Write

## When to use

Create, correct or remove memory items.

## Tools

`admit_preview`, `admit_preview_batch`, `store`, `update`, `invalidate`, `discard`, `correct`, `batch`, `validate`, `canonical_put`, `shared_store`, `shared_discard`, `scratchpad_write`, `scratchpad_clear`

## Examples

```
store({"item": {"id": "demo-note", "gist": "prefers terse answers"}, "category": "persona", "epistemic_kind": "fact"})
discard({"item_id": "demo-note", "reason": "superseded", "confirm": true})
```

## Exact arguments

Call `tool_describe` with a tool name from the list above for the exact arguments. For example: `tool_describe("store")`.
