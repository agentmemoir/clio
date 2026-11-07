# Memtree

## When to use

Read hierarchical or temporal memory.

## Tools

`memtree_query`, `memtree_get`, `temporal_history`

## Examples

```
memtree_query({"limit": 10})
temporal_history({"target": "preference:brevity_affinity"})
```

## Exact arguments

Call `tool_describe` with a tool name from the list above for the exact arguments. For example: `tool_describe("memtree_query")`.
