# `tools/list` pagination

Clio pages `tools/list`. A client reads one page at a time and asks for the next page with the cursor the server returns. The page-size request key is a Clio extension, not part of the Model Context Protocol request shape. A client that sends no extension key still gets valid pages, so a spec-only client walks the whole listing without changes.

## Page size

- Absent, the page holds 25 tools.
- A request may set `params._meta["com.agentmemoir/pageSize"]` to an integer of at least 1.
- A value above 100 is clamped to 100.
- A value below 1, a non-integer, or a value of another JSON type answers `-32602` (`Invalid params`).

Tools stay in catalog order. The page size governs one page only.

## Cursor

A page that ends before the view does carries `nextCursor`. The last page omits the key.

The cursor is opaque. Its shape is:

```
base64url(no padding) of [1, "<digest>", <page size>, <next offset>]
```

- `1` is the format version. Any other version answers `-32602`.
- `<digest>` is the visible-view digest, written as `fnv1a64:<16 lowercase hex digits>`.
- `<page size>` is the size the page was cut with.
- `<next offset>` is the index of the first tool of the next page.

The digest covers three inputs:

1. the catalog hash of the visible definitions, in catalog order;
2. the selected tool profile and the groups it resolves to;
3. the discovery token, which is the effective `mcp.revision` and the pinned protocol revision when no revision is set.

The digest binds a cursor to one view. A change to a tool definition, a profile, or the discovery token makes old cursors answer `-32602` instead of returning a page from a different view.

The cursor holds no secret and no session id. The server keeps no per-client pagination state.

## Refusals

The server answers `-32602` (`Invalid params`) when:

- the cursor is not valid base64url, is not a JSON four-field tuple, or carries a field of the wrong type;
- the cursor format version is not `1`;
- the cursor digest does not match the current view;
- the page size in the request differs from the page size in the cursor;
- the cursor offset is past the end of the view.

A request may repeat the page size that the first request used, or omit it. Both are accepted.

## Example walk

```json
{"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {"_meta": {"com.agentmemoir/pageSize": 50}}}
```

The answer carries at most 50 tools and, when more remain, a `nextCursor`. The next request sends the cursor and the same page size:

```json
{"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {"cursor": "<nextCursor>", "_meta": {"com.agentmemoir/pageSize": 50}}}
```

A walk visits every visible tool exactly once, in catalog order, with no duplicate, omission, or reorder.

## Repeatability

Identical requests against an unchanged view return byte-identical result bytes, cursors included. The result carries no time, randomness, or session id.
