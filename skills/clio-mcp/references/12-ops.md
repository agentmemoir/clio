# Ops

## When to use

Diagnose, verify or repair operations.

## Tools

`diagnose`, `verify`, `doctor`, `repair`, `reindex`, `stats`

## Examples

Start with the read-only checks; `doctor` prints a plan unless told otherwise, and `repair` and `reindex` need `confirm` before they change anything.

```
diagnose({})
reindex({"target": "lexical", "confirm": true})
```

## Exact arguments

Call `tool_describe` with a tool name from the list above for the exact arguments. For example: `tool_describe("diagnose")`.
