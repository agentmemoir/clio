# Config

## When to use

Read or change configuration.

Only the `all` preset lists this group, so `tools/list` may not show it under another preset, and a tool a preset does not list stays callable by name.

## Tools

`config_get`, `config_set`, `config_profiles`, `config_profile_apply`

## Examples

`config_set` validates the value before it applies, and `scope` defaults to `session`.

```
config_get({})
config_set({"path": "log.level", "value": "debug"})
```

## Exact arguments

Call `tool_describe` with a tool name from the list above for the exact arguments. For example: `tool_describe("config_get")`. `tool_describe` exists only when discovery is enabled (`--discovery on` or `mcp.discovery: true`).
