# Task

## When to use

Record task state and failure history.

## Tools

`task_upsert`, `task_get`, `task_history`, `failure_record`, `failures_for_task`

## Examples

```
task_upsert({"task_id": "tsk-1", "definition": "deploy the service", "status": "open"})
failure_record({"task_id": "tsk-1", "attempt_n": 1, "what_failed": "timeout", "lesson": "raise the deadline"})
```

## Exact arguments

Call `tool_describe` with a tool name from the list above for the exact arguments. For example: `tool_describe("task_upsert")`.
