# Sync

## When to use

Exchange memory between peers.

The default `minimal` preset does not list this group, so `tools/list` may not show it, and a tool a preset does not list stays callable by name.

## Tools

`sync_push`, `sync_pull`, `sync_serve`, `sync_ack_skip`, `sync_status`, `sync_key_status`

## Examples

`sync_status` reports the state of one peer and makes no connection. `sync_key_status` reports the keys this node holds and never returns key material.

```
sync_status({})
sync_key_status({})
```

`sync_push` and `sync_pull` need a `remote` endpoint and a `token`. `sync_serve` starts a peer listener. `sync_ack_skip` accepts one dead-lettered mutation and acts only when the call confirms, so it reports a dry run by default.

## Exact arguments

Call `tool_describe` with a tool name from the list above for the exact arguments. For example: `tool_describe("sync_status")`. `tool_describe` exists only when discovery is enabled (`--discovery on` or `mcp.discovery: true`).
