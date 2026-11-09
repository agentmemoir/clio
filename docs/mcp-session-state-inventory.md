# MCP session state inventory (issue 537)

## Purpose and scope

This file classifies every field the MCP protocol session owned at the time it was written, and it is kept as the historical record of that pass. The activation has since landed: the session store, the session counter, the `MCP-Session-Id` header in both directions, the `initialize` handshake route, and the `--sessions` CLI switch are all removed, so the rows that classify them as lifecycle/bookkeeping now describe a path that no longer exists. The classification is unchanged and still the input the activation used; only the rows marked removed below no longer name live code. `McpState` is the process-wide runtime rather than the session, so it appears here only for the per-request fork, for the one mutable process-wide field a request can write, and for the statement that it holds no session field. The transport now serves revision `2026-07-28`, whose dispatcher keeps no per-call state.

## Method

Each field was read at its definition in this repository at this commit, and its readers and writers were then traced. A line number is a pointer, not a contract: a field that moves keeps its meaning and loses its line. Re-derive every row with the commands in `## How to re-check` before relying on a line here.

## Findings

The classification rule is one sentence. Domain state answers a request; lifecycle and bookkeeping state exists only to create, find, or end a session.

| Field | Owner | Readers | Writers | Classification |
| --- | --- | --- | --- | --- |
| `HttpShared::sessions` (`http_guard.rs:51`) | `http_guard` (`http_guard.rs:48`) | `is_live` (`http_guard.rs:63`) and `handle_delete` (`http_guard.rs:118`) | `mint_session` (`http_guard.rs:157`) | lifecycle/bookkeeping — an id → minting-identity map, so the value is an identity and never a tool result  **Removed by activation: the field no longer exists.** |
| `HttpShared::session_counter` (`http_guard.rs:52`) | `http_guard` (`http_guard.rs:48`) | `mint_session` (`http_guard.rs:145`) | `mint_session` only | lifecycle/bookkeeping — a monotonic counter that feeds id minting and nothing else  **Removed by activation: the field no longer exists.** |
| `HttpConfig::sessions_enabled` (`http.rs:103`) | `http` | the session gate at `http.rs:312` and the mint branch at `http.rs:330` | `http_config` at `mcp_cli.rs:355`; default `false` at `http.rs:128` | lifecycle/bookkeeping — a deployment switch for the lifecycle, which no tool body reads  **Removed by activation: the field no longer exists.** |
| `MCP-Session-Id` request header | `http` and `http_guard` | `http.rs:314` and `http_guard.rs:123` | client | lifecycle/bookkeeping — the transport id, never forwarded into dispatch |
| `MCP-Session-Id` response header | `http` | client | `http.rs:332` | lifecycle/bookkeeping — written once after a successful `initialize` |
| `initialize` handshake route (`protocol.rs:120`) | `protocol` | the route arm, and the mint branch at `http.rs:330` | client request | lifecycle/bookkeeping — the one method that creates a session; the candidate refuses it (`stateless.rs:66`)  **Removed by activation: the field no longer exists.** |
| `--sessions` CLI switch, declared at `cli_command_registry_surface_mcp.rs:46` | `clio` | read into the HTTP config by `http_config` (`mcp_cli.rs:355`), and the value check `refuse_sessions_value` (`mcp_cli.rs:370`, refusal at `:374`) | operator | lifecycle/bookkeeping — turns the whole lifecycle on or off  **Removed by activation: the field no longer exists.** |
| `Principal::session_identity` (`principal.rs:73`), the label a session binds to | `principal` | `http.rs:283` | derived from the record label, so no separate write | domain identity — it names who is calling and outlives any session |
| `McpState::principal` (`runtime.rs:225`) | `runtime` | the authorization checks, for example `runtime.rs:336` | `with_principal` (`runtime.rs:317`), called by `scoped_state` (`http.rs:324`) | domain identity — the single per-request fork of the process state, held for one call and never stored per session |
| `McpState::config` (`runtime.rs:193`), the process-wide effective-config runtime | `runtime` | `dispatch_read` (`config_tools.rs:41`) behind `config_get`, `config_profiles`, and `ranking_env_get`, the tool-profile read behind every `tools/list` (`protocol.rs:343`), and `summarize` (`summarize_tools.rs:77`) | `config_set` through `dispatch_write` (`config_tools.rs:54`, the arm at `:56`), which writes the overlay its `scope` names, the session scope included (`config_ops.rs:81`) | domain — process-wide, mutable, not session-keyed |
| `McpState` as a whole (`runtime.rs:71`) | `runtime` | every tool handler | opened once on the serving path | domain — process-wide and not session-owned; it holds no session, session-id, connection, or transport field |

The protocol session owns no domain state. That conclusion follows from the table rather than from a preference. The session map holds one thing per session: the identity that minted the id. The counter, the config switch, the two header directions, the handshake route, and the CLI switch each exist only to create, find, or end a session. So the domain-state inventory for the protocol session is empty, and the candidate creates no handle for it. Independence therefore covers calls that write no process-wide state: a `config_set` write under the session scope lands in `McpState::config` (`config_ops.rs:81`) and later calls read it (`config_tools.rs:41`), so a config write is visible to later calls by design, and that live behavior is unchanged here.

The one session-keyed domain store that does exist is the compliance scratchpad, keyed by `(session, bank)` at `scratchpad.rs:72`. Its session value comes from a normal tool argument: `session_of` reads the `session` argument at `additive_tools.rs:92` and each scratchpad handler passes it (`additive_tools.rs:102`, `:121`, `:132`). An omitted argument falls back to the named default `default` at `additive_tools.rs:93`. A pad carries a four-hour inactivity TTL (`scratchpad.rs:39`), but eviction is lazy: `evict_expired` (`scratchpad.rs:100`) runs at the top of the scratchpad's own write, read, and clear calls (`scratchpad.rs:129`, `:170`, `:222`), so an abandoned pad survives until some other scratchpad call runs. The number of distinct pads is unbounded, and the session string is validated only as non-empty (`scratchpad.rs:116`). Only per-pad bytes (`scratchpad.rs:35`, checked at `scratchpad.rs:137`) and per-pad key count (`scratchpad.rs:37`, checked at `scratchpad.rs:146`) are capped, at 64 KiB and 100 keys. This session key is therefore a normal explicit argument with a stated lifetime and validation, so it needs no handle, no handshake and no header.

The candidate is `crates/clio-mcp/src/stateless.rs`. Its dispatch takes a shared process state and one message (`stateless.rs:76`) and reads no session field from either. It keeps no per-call state, mints nothing, and refuses the handshake methods, which are listed at `stateless.rs:66` and refused at `stateless.rs:83`. There is therefore no lifecycle or bookkeeping dependency to remove from it. Its tests show that a first message is served without a handshake, that the entry point takes only the shared state and one message, that no answer envelope carries a session key, and that repeated calls agree across two independently built states.

The candidate is reachable only from this crate's tests: `lib.rs:103` is the `#[cfg(test)]` attribute above `mod stateless;` at `lib.rs:104`, `lib.rs:105` is the attribute above `mod stateless_fixtures;` at `lib.rs:106`, and `stateless.rs:283` is the attribute above `mod stateless_tests;` at `stateless.rs:285`, so no production build compiles any of them. No runtime revision selector exists: the served revision is the constant at `lib.rs:112`, and no CLI flag or environment value names the candidate revision.

## What activation removed

Activation removed the session store, the session counter, the `MCP-Session-Id` header, the handshake route, and the `--sessions` switch in one change, and that change has landed: the rows above that name `HttpShared::sessions`, `HttpShared::session_counter`, `HttpConfig::sessions_enabled`, the `initialize` handshake route, and the `--sessions` CLI switch are historical, and the paths and line numbers they cite no longer exist. This inventory was that change's input: it named each field the change would delete and showed that no domain value depends on one. What survived is the domain state the table also lists, which is why the activation needed no data migration.

## How to re-check

```
rg -n 'sessions: Mutex|session_counter' crates/clio-mcp/src/http_guard.rs
rg -n 'sessions_enabled|mcp-session-id|MCP-Session-Id' crates/clio-mcp/src/http.rs
rg -n 'refuse_sessions_value|sessions_enabled' crates/clio/src/mcp_cli.rs
rg -n 'MAX_TOTAL_BYTES|MAX_KEYS|const TTL|sessions: Mutex' crates/clio-compliance/src/scratchpad.rs
rg -n 'pub config:|config.lock' crates/clio-mcp/src/runtime.rs crates/clio-mcp/src/config_tools.rs
```

The three commands naming a removed field (`sessions: Mutex`, `session_counter`, `sessions_enabled`, `refuse_sessions_value`, `mcp-session-id`) now report nothing, and that is the expected answer after activation: nothing on the serving path owns a session any more.
