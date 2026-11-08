# Compliance

## When to use

Track subject requests and erasure.

The default `minimal` preset does not list this group, so `tools/list` may not show it, and a tool a preset does not list stays callable by name.

## Tools

`subject_request_start`, `subject_request_status`, `subject_request_retry`, `subject_request_copy`, `subject_request_exception`, `subject_request_events`, `subject_request_path_complete`, `subject_request_close`, `subject_request_peer_status`, `subject_request_peer_retry`, `subject_request_peer_disposition`, `erase_request`

## Exact arguments

These tools erase data or move a subject request forward, and the erasing call needs a confirmation before it acts. The caller must call `tool_describe` with the tool name to read the exact arguments before the first call. For example: `tool_describe("erase_request")`. `tool_describe` exists only when discovery is enabled (`--discovery on` or `mcp.discovery: true`).

This file names the tools and points at their descriptions. It shows no call and no argument value, because a copied call can erase data. Read the description first, then agree the subject and the scope with the operator before the call.
