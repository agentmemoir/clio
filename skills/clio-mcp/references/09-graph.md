# Graph

## When to use

Read or add memory relationships.

## Tools

`graph_link`, `graph_query`

## Examples

```
graph_link({"source_id": "itm-a", "target_id": "itm-b", "relationship": "supports"})
graph_query({"seed_id": "itm-a"})
```

## Exact arguments

Call `tool_describe` with a tool name from the list above for the exact arguments. For example: `tool_describe("graph_link")`.

The default profile lists this group. The `coding` and `ops` profiles do not list these tools, but a tool stays callable when the listing does not show it: call `tool_describe` with the tool name for the arguments, then call `tool_invoke` with that name and the arguments.
