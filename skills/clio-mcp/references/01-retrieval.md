# Retrieval

## When to use

Find memory or build grounded context.

## Tools

`intent_gate`, `retrieve`, `compose_context`, `associations`

## Examples

```
retrieve({"query": "deploy steps"})
compose_context({"budget_tokens": 1000})
```

## Exact arguments

Call `tool_describe` with a tool name from the list above for the exact arguments. For example: `tool_describe("retrieve")`.
