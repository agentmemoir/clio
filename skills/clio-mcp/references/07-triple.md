# Triple

## When to use

Record or query factual triples.

## Tools

`triple_add`, `triple_query`, `triple_end`

## Examples

```
triple_add({"subject": "alice", "predicate": "lives_in", "object": "boston", "epistemic_kind": "fact"})
triple_query({"subject": "alice"})
```

## Exact arguments

Call `tool_describe` with a tool name from the list above for the exact arguments. For example: `tool_describe("triple_add")`.
