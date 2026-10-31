# MCP surfaces (pinned revision `2025-11-25`)

The `clio-mcp` crate publishes machine-readable `tool_schema`s for the Core tool catalog and exposes the write/mutate **and** read/retrieve/compose sets over **stdio** and **Streamable HTTP** with identical tool names and semantics. Gates run inside the in-process tools on every path; transports cannot bypass admission.

## Commands

- `clio mcp schema-export [--out FILE]` (alias `--path FILE`) — print (or write) the `tool_schema` pack (JSON Schema per Core tool, protocol-revision stamp, catalog hash).
- `clio mcp stdio [--db PATH] [--backend sqlite|postgres] [--bank BANK]` — run the MCP stdio transport (newline-delimited JSON-RPC on stdout; logs on stderr only).
- `clio mcp http --bind ADDR [--db PATH] [--auth-token TOKEN] [--token-file PATH] [--sessions]` — run the Streamable HTTP transport on `POST /mcp`. Loopback binds are allowed without a credential (dev mode); a non-loopback bind refuses to start without one. The token comes from `--auth-token`, else from the `CLIO_MCP_TOKEN` environment variable (the flag wins when both are set; an empty environment value counts as unset). `--sessions` mints and enforces `MCP-Session-Id` lifecycle; on a credential-bound server every session belongs to the credential that minted it, and presenting it as anyone else is refused exactly like an unknown session id.

## Network credentials

`--token-file PATH` loads the token store that `clio auth token add` manages, once, at server start. Each record names an actor and the banks it may reach, and a request presenting that record's secret acts as that actor and reaches only those banks.

Three rules, all refusals rather than fallbacks:

- A declared `actor` that differs from the record is refused. An equal value is accepted; an absent one uses the record's actor. The point is that the audit rows cannot record an actor the caller did not present.
- A bank outside the record's list is refused, whether the caller named it or the default resolved to it. There is no fallback to a listed bank, because a fallback would write where the caller did not ask.
- The shared-bank deployment gate is unchanged and still applies on top of the record's list. A record naming `shared` does not switch that gate on.

The start fails closed: a path that is absent, unreadable, malformed, readable by anyone but its owner, or holding no record refuses the start rather than serving a 401 for every request. Missing and unknown secrets both answer `401` with the same body, so a refusal discloses nothing about which credentials exist.

`--auth-token TOKEN` remains accepted and now acts as actor `agent` with access to every bank. Reads and writes keep working. `export` and `erase_request` through it now fail closed, because only `harness` or `system` carry that privilege; add a scoped credential with `clio auth token add --actor harness` and pass `--token-file` if you need them.

`bank`/`actor` in the request arguments are still accepted over HTTP, but on a credential-bound server they are checked against the credential rather than trusted. The local paths (`stdio`, the CLI) have no credential edge and keep the unbound contract: the process owner already holds the store.

## Transport exposure

What a credential proves after authentication: the actor and the bank list its record names, nothing more. A request cannot widen them by argument; the rules above refuse both.

Two exposures an operator should know about:

- A secret supplied as a command-line flag is visible in the process list (`ps`) on many hosts. Prefer the `CLIO_MCP_TOKEN` environment variable or a `--token-file` store; the flag stays accepted. An environment value is not in the process list, but it is still readable by other processes of the same user, so a token file remains the strongest of the three sources.
- Neither this transport nor the sync transport terminates TLS. They speak cleartext HTTP and authenticate per request. Publish a port only behind a trusted network boundary, and prefer a reverse proxy that terminates TLS if the port must be reachable off-host.

The Clio-managed PostgreSQL setup is unchanged by any of this: loopback-only, no password (`trust`), by design for local development and the Compose flow.

## Bound write tools

`admit_preview`, `store`, `update`, `invalidate`, `discard`, `consolidate`, `persona_put_stable`, `persona_observe_preference`, `task_upsert`, `failure_record`, `triple_add`, `triple_end`, `belief_observe`, `graph_link`, `sync_push`, `sync_pull`, `sync_serve`, `sync_ack_skip`. A sync transfer names the presenting credential as `principal` in the response; an unbound local call omits the field.

## Bound read tools

`intent_gate`, `retrieve`, `compose_context`, `associations`, `get`, `get_snapshot`, `get_gist`, `maintenance_status`, `persona_get`, `task_get`, `task_history`, `failures_for_task`, `memtree_query`, `memtree_get`, `temporal_history`, `triple_query`, `belief_history`, `graph_query`, `inspect`, `sync_status`, `sync_key_status`. Every read tool except the sync routes accepts `bank`, `actor`, and `explain` (§4.9.4.G). The sync routes take `banks` and refuse any bank outside the credential's list or a `shared` bank the deployment has not enabled.

## Raw-ingest binding decision

Raw conversation-turn ingestion remains a library API (`clio_write::ingest_raw`) and is intentionally not published as an MCP tool. Native callers that already own turn chunking can call that API directly; the existing `store` binding remains the MCP write surface. Keeping raw ingest library-only avoids a second extraction and egress contract. A future MCP wrapper requires a reviewed versioned `tool_schema` entry and explicit approval. No external non-native caller has demonstrated a need for unchunked turn extraction over MCP, so no schema draft is published; a draft would be reconsidered only when such a caller appears.

`bank`/`actor` are accepted per call; per-call args win over the connection defaults. `dry_run` is defined where the mutator defines it (`store` skips only the durable write; `discard` stays a dry-run until `confirm: true`).

## Conformance suite

Run the pinned-revision conformance matrix locally:

```sh
cargo test -p clio-mcp --test conformance
cargo test -p clio-mcp --test mcp_read_conformance
cargo test -p clio-mcp --test mcp_read_mock
```

The suites cover: initialize handshake, `tools/list` bound tool set, a tools/call matrix for every bound write tool (happy path + gated-reject samples) and every bound read tool over both transports, each with parity against the in-process dispatcher, bad-Origin → 403, missing/wrong auth → 401 when a token is configured, the optional session lifecycle, stdio stdout purity, a mock client that simulates dropped connections / mid-stream aborts / dropped session tokens, and the `2025-11-25` revision assertion. CI runs them as the `mcp-conformance-2025-11-25` job.