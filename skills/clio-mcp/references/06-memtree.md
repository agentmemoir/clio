# Memtree

## When to use

Read hierarchical or temporal memory.

## Tools

`memtree_query`, `memtree_get`, `temporal_history`

## Examples

```
memtree_query({"task_id": "tsk-1"})
temporal_history({"target": "preference:communication_style"})
```

## Exact arguments

Call `tool_describe` with a tool name from the list above for the exact arguments. For example: `tool_describe("memtree_query")`.
