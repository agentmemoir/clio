# Belief

## When to use

Record or inspect beliefs.

## Tools

`belief_observe`, `belief_history`

## Examples

```
belief_observe({"proposition": "Friday deploys are risky", "confidence": 0.8, "evidence_ref": "incident-12", "source_type": "user_stated"})
belief_history({"belief_id_or_proposition": "Friday deploys are risky"})
```

## Exact arguments

Call `tool_describe` with a tool name from the list above for the exact arguments. For example: `tool_describe("belief_observe")`.
