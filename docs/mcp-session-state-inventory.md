# MCP session state inventory (issue 537)

## Purpose and scope

This file classifies every field the MCP protocol session owns today, so a later stateless candidate can be shown to need no handshake, no session id, and no hidden connection state. It covers only the wire and protocol session: the fields a request could reach through a connection or a session id. `McpState` is the process-wide runtime rather than the session, so it appears here only for the per-request fork and for the statement that it holds no session field. This file records what exists; it removes nothing.

## Method

Each field was read at its definition in this repository at this commit, and its readers and writers were then traced. A line number is a pointer, not a contract: a field that moves keeps its meaning and loses its line. Re-derive every row with the commands in `## How to re-check` before relying on a line here.

## Findings

The classification rule is one sentence. Domain state answers a request; lifecycle and bookkeeping state exists only to create, find, or end a session.

| Field | Owner | Readers | Writers | Classification |
| --- | --- | --- | --- | --- |
| `HttpShared::sessions` (`http_guard.rs:51`) | `http_guard` (`http_guard.rs:48`) | `is_live` (`http_guard.rs:63`) and `handle_delete` (`http_guard.rs:118`) | `mint_session` (`http_guard.rs:157`) | lifecycle/bookkeeping — an id → minting-identity map, so the value is an identity and never a tool result |
| `HttpShared::session_counter` (`http_guard.rs:52`) | `http_guard` (`http_guard.rs:48`) | `mint_session` (`http_guard.rs:145`) | `mint_session` only | lifecycle/bookkeeping — a monotonic counter that feeds id minting and nothing else |
| `HttpConfig::sessions_enabled` (`http.rs:103`) | `http` | the session gate at `http.rs:312` and the mint branch at `http.rs:330` | `http_config` at `mcp_cli.rs:355`; default `false` at `http.rs:128` | lifecycle/bookkeeping — a deployment switch for the lifecycle, which no tool body reads |
| `MCP-Session-Id` request header | `http` and `http_guard` | `http.rs:314` and `http_guard.rs:123` | client | lifecycle/bookkeeping — the transport id, never forwarded into dispatch |
| `MCP-Session-Id` response header | `http` | client | `http.rs:332` | lifecycle/bookkeeping — written once after a successful `initialize` |
| `initialize` handshake route (`protocol.rs:120`) | `protocol` | the route arm, and the mint branch at `http.rs:330` | client request | lifecycle/bookkeeping — the one method that creates a session; the candidate refuses it (`stateless.rs:66`) |
| `--sessions` CLI switch (`mcp_cli.rs:355`) | `clio` | `http_config` (`mcp_cli.rs:351`) and the value check `refuse_sessions_value` (`mcp_cli.rs:370`, refusal at `:374`) | operator | lifecycle/bookkeeping — turns the whole lifecycle on or off |
| `Principal::session_identity` (`principal.rs:73`), the label a session binds to | `principal` | `http.rs:283` | derived from the record label, so no separate write | domain identity — it names who is calling and outlives any session |
| `McpState::principal` (`runtime.rs:225`) | `runtime` | the authorization checks, for example `runtime.rs:336` | `with_principal` (`runtime.rs:317`), called by `scoped_state` (`http.rs:324`) | domain identity — the single per-request fork of the process state, held for one call and never stored per session |
| `McpState` as a whole (`runtime.rs:71`) | `runtime` | every tool handler | opened once on the serving path | domain — process-wide and not session-owned; it holds no session, session-id, connection, or transport field |

The protocol session owns no domain state. That conclusion follows from the table rather than from a preference. The session map holds one thing per session: the identity that minted the id. The counter, the config switch, the two header directions, the handshake route, and the CLI switch each exist only to create, find, or end a session. So the domain-state inventory for the protocol session is empty, and the candidate creates no handle for it.

The one session-keyed domain store that does exist is the compliance scratchpad, keyed by `(session, bank)` at `scratchpad.rs:72`. Its session value comes from a normal tool argument: `session_of` reads the `session` argument at `additive_tools.rs:92` and each scratchpad handler passes it (`additive_tools.rs:102`, `:121`, `:132`). An omitted argument falls back to the named default `default` at `additive_tools.rs:93`. A pad expires after a fixed inactivity TTL of four hours (`scratchpad.rs:39`, evicted at `scratchpad.rs:100`). It enforces a 64 KiB per-entry cap (`scratchpad.rs:35`, checked at `scratchpad.rs:137`) and a 100-key per-key cap (`scratchpad.rs:37`, checked at `scratchpad.rs:146`). This session key is therefore a normal explicit argument with a specified lifetime and validation, so it needs no handle, no handshake and no header.

The candidate is `crates/clio-mcp/src/stateless.rs`. Its dispatch takes a shared process state and one message (`stateless.rs:76`) and reads no session field from either. It keeps no per-call state, mints nothing, and refuses the handshake methods, which are listed at `stateless.rs:66` and refused at `stateless.rs:83`. There is therefore no lifecycle or bookkeeping dependency to remove from it. Its tests prove that a first message is served without a handshake, that the entry point takes only the shared state and one message, and that every answer carries no session identity.

The candidate is reachable only from this crate's tests. The module is registered under `#[cfg(test)]` at `lib.rs:103` and its fixtures at `lib.rs:105`, so no production build compiles it. No runtime revision selector exists: the served revision is the constant at `lib.rs:112`, and no CLI flag or environment value names the candidate revision.

## What a later step may cover

Activation would remove the session store, the `MCP-Session-Id` header, the handshake route, and the `--sessions` switch in one change. That change is out of scope here. This inventory is its input: it names each field the change would delete and shows that no domain value depends on one. Nothing in this file deletes, disables, or deprecates any of them.

## How to re-check

```
rg -n 'sessions: Mutex|session_counter' crates/clio-mcp/src/http_guard.rs
rg -n 'sessions_enabled|mcp-session-id|MCP-Session-Id' crates/clio-mcp/src/http.rs
rg -n 'refuse_sessions_value|sessions_enabled' crates/clio/src/mcp_cli.rs
rg -n 'MAX_TOTAL_BYTES|MAX_KEYS|const TTL|sessions: Mutex' crates/clio-compliance/src/scratchpad.rs
```
