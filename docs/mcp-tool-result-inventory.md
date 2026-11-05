# MCP tool result inventory (issue 493)

## Purpose and scope

This file is the committed record of which MCP tool results are already structured, one row per bound tool. It owns one thing: for each tool, the handler that builds the result, the kind of value it builds, and whether an MCP output schema could describe that value. It does not declare an output schema, and it does not change any result, handler, envelope or schema. Nothing in this file is a schema: no schema object, no schema fragment, and no schema field appears here. The question it answers is only "what does the tool already return today".

## Method

The 86 tool names come from `crates/clio-types/src/mcp_tools.rs`. `BOUND_WRITE_TOOLS` holds 50 names (line 26). `BOUND_READ_TOOLS` holds 36 names (line 80). `bound_tools()` at line 121 joins the two lists. Every result is wrapped by the envelope in `crates/clio-mcp/src/protocol.rs:212-216`. That envelope puts the handler value into `structuredContent`, and its serialized text into `content`. So every tool already returns a JSON object. Each row cites the handler that the dispatch reaches. Where a callee forwards or envelopes the value, the row names that callee. For example, the scratchpad reader forwards to the compliance scratchpad. The operator reports gain `ok` from the ops envelope. Two limits apply to this record. Handler line numbers drift when a handler moves. Typed payloads built in other crates are classified from the call site. The shared `ToolResult<Value>` envelope (`write_tools.rs:111`) is on every write tool routed through `tool_result`. The exceptions are `repair` and `reindex` (which build `ok` in `ops_tools`), and `store` and `batch` (which wrap earlier). The shared envelope is not counted here. Any other key merged into the handler value inside `clio-mcp` is counted. `typed struct` means the value is `serde_json::to_value` of a `#[derive(Serialize)]` type. `hand-built object` means `json!`/`Map` assembly, including a typed base that then gains inserted keys. `delegated` means the handler returns another tool's dispatch result. The `Schema fit` column has four values. `direct` means one typed value covers the whole result. `partial-base` means a typed value plus keys inserted after serialization, in a single shape. `partial-shapes` means the handler builds more than one result shape in the same row. One example is a preview branch and a confirmed branch.

## Findings

The inventory is not empty. All 86 bound tools already return a JSON object inside `structuredContent`. So every row has a value that an output schema could describe. 27 tools return a value serialized from a typed serde struct. 56 tools return a hand-built `json!`/`Map` object. 3 tools delegate to another tool (`shared_store`, `shared_discard`, `shared_retrieve`). By schema fit the split is 27 `direct`, 15 `partial-base`, 1 `partial-shapes` and 43 `by hand`. Every published tool carries an `outputSchema`: output schemas predate this inventory and are attached to each tool entry from `crates/clio-mcp/output_schemas.json`, so the `structuredContent` populated at `protocol.rs:215` is described by a declared schema as well as by the `inputSchema` built at `schema_defs.rs:76`.

## Write tools (50 rows, in `mcp_tools.rs` source order)

| Tool | Handler | Result source | Schema fit |
| --- | --- | --- | --- |
| `admit_preview` | `admit_preview` — `store_write.rs:168` → `store_path` (`store_write.rs:67`) | hand-built object | partial-base |
| `store` | `store_envelope` — `store_write.rs:183` → `store` (`store_write.rs:174`) | hand-built object | partial-base |
| `update` | `update` — `mutator_tools.rs:54` | hand-built object | by hand |
| `invalidate` | `invalidate` — `mutator_tools.rs:104` → `end_outcome_json` (`write_tools.rs:336`) | hand-built object | by hand |
| `discard` | `discard` — `mutator_tools.rs:126` | hand-built object | by hand |
| `correct` | `correct` — `mutator_tools.rs:177` | hand-built object | partial-shapes |
| `consolidate` | `consolidate` — `mutator_tools.rs:379` | typed struct | direct |
| `persona_put_stable` | `persona_put_stable` — `mutator_record_tools.rs:34` | hand-built object | by hand |
| `persona_observe_preference` | `persona_observe` — `mutator_record_tools.rs:83` | hand-built object | by hand |
| `task_upsert` | `task_upsert` — `mutator_record_tools.rs:130` | hand-built object | by hand |
| `failure_record` | `failure_record` — `mutator_record_tools.rs:189` | hand-built object | by hand |
| `triple_add` | `triple_add` — `write_tools_handlers.rs:37` | hand-built object | partial-base |
| `triple_end` | `triple_end` — `write_tools_handlers.rs:94` → `end_outcome_json` (`write_tools.rs:336`) | hand-built object | by hand |
| `belief_observe` | `belief_observe` — `write_tools_handlers.rs:117` | typed struct | direct |
| `graph_link` | `graph_link` — `write_tools_handlers.rs:147` | typed struct | direct |
| `erase_request` | `erase_request` — `mutator_tools.rs:306` → `retry_value` (`subject_request_tools.rs:150`) | hand-built object | partial-base |
| `batch` | `batch_tool` — `batch_tools.rs:59` → `run_batch` (`batch_tools.rs:63`) | hand-built object | by hand |
| `scratchpad_write` | `scratchpad_write` — `additive_tools.rs:97` | hand-built object | by hand |
| `scratchpad_read` | `scratchpad_read` — `additive_tools.rs:118` | hand-built object | by hand |
| `scratchpad_clear` | `scratchpad_clear` — `additive_tools.rs:129` | hand-built object | by hand |
| `canonical_put` | `canonical_put` — `additive_tools.rs:143` | hand-built object | partial-base |
| `shared_store` | `shared_store` — `additive_tools.rs:283` → `write_tools::dispatch` | delegated | by hand |
| `shared_discard` | `shared_discard` — `additive_tools.rs:297` → `write_tools::dispatch` | delegated | by hand |
| `validate` | `validate_tool` — `additive_tools.rs:303` | typed struct | direct |
| `hygiene_clean` | `hygiene_clean_tool` — `hygiene_tools.rs:75` | typed struct | direct |
| `export` | `export_tool` — `portability_tools.rs:64` | hand-built object | by hand |
| `import` | `import_tool` — `portability_tools.rs:195` | hand-built object | by hand |
| `import_provider` | `import_provider_tool` — `portability_tools.rs:233` → `import_provider_mem0` (`provider_adapter.rs:62`) → else `import_provider_stub` (`portability_tools.rs:246`) | hand-built object | by hand |
| `review_export` | `review_export_tool` — `portability_tools.rs:147` | hand-built object | by hand |
| `repair` | `repair_result` — `ops_tools.rs:218` | hand-built object | partial-base |
| `reindex` | `reindex_result` — `ops_tools.rs:285` | hand-built object | partial-base |
| `sync_push` | `transfer` — `sync_transfer.rs:57` | hand-built object | by hand |
| `sync_pull` | `transfer` — `sync_transfer.rs:57` | hand-built object | by hand |
| `sync_serve` | `sync_serve_with` — `sync_tools.rs:210` | hand-built object | by hand |
| `sync_ack_skip` | `ack_skip_tool` — `sync_ack.rs:76` | hand-built object | by hand |
| `retention_profile_set` | `profile_set` — `retention_tools.rs:97` | hand-built object | by hand |
| `config_set` | `dispatch_write` — `config_tools.rs:51` (arm `:56`) | typed struct | direct |
| `config_profile_apply` | `dispatch_write` — `config_tools.rs:51` (arm `:62`) | typed struct | direct |
| `ranking_env_set` | `dispatch_write` — `config_tools.rs:51` (arm `:73`) | typed struct | direct |
| `subject_request_start` | `start` — `subject_request_tools.rs:75` | typed struct | direct |
| `subject_request_status` | `status` — `subject_request_tools.rs:101` | typed struct | direct |
| `subject_request_retry` | `retry` — `subject_request_tools.rs:108` → `retry_value` (`subject_request_tools.rs:150`) | hand-built object | partial-base |
| `subject_request_copy` | `copy` — `subject_request_tools.rs:161` | typed struct | direct |
| `subject_request_exception` | `exception` — `subject_request_tools.rs:181` | typed struct | direct |
| `subject_request_events` | `events` — `subject_request_tools.rs:193` | hand-built object | by hand |
| `subject_request_path_complete` | `path_complete` — `subject_request_tools.rs:223` | typed struct | direct |
| `subject_request_close` | `close` — `subject_request_tools.rs:235` | typed struct | direct |
| `subject_request_peer_status` | `peer_status` — `subject_request_tools.rs:247` | typed struct | direct |
| `subject_request_peer_retry` | `peer_retry` — `subject_request_tools.rs:256` | typed struct | direct |
| `subject_request_peer_disposition` | `peer_disposition` — `subject_request_tools.rs:282` | typed struct | direct |

## Read tools (36 rows, in `mcp_tools.rs` source order)

| Tool | Handler | Result source | Schema fit |
| --- | --- | --- | --- |
| `intent_gate` | `intent_gate` — `read_retrieve.rs:89` | hand-built object | by hand |
| `retrieve` | `retrieve` — `read_retrieve.rs:120` → `retrieve_at` (`read_retrieve.rs:131`) | hand-built object | partial-base |
| `compose_context` | `compose_context` — `read_retrieve.rs:249` → `compose_context_at` (`read_retrieve.rs:257`) | hand-built object | partial-base |
| `associations` | `associations` — `read_retrieve.rs:345` | hand-built object | by hand |
| `get` | `get_item` — `read_tools.rs:153` | hand-built object | partial-base |
| `get_snapshot` | `get_snapshot` — `read_tools.rs:175` | hand-built object | by hand |
| `get_gist` | `get_gist` — `read_tools.rs:188` | hand-built object | by hand |
| `maintenance_status` | `maintenance_status` — `read_tools.rs:204` | hand-built object | partial-base |
| `persona_get` | `persona_get` — `read_tools.rs:236` | hand-built object | by hand |
| `task_get` | `task_get` — `read_history.rs:68` | hand-built object | by hand |
| `task_history` | `task_history` — `read_history.rs:88` | hand-built object | by hand |
| `failures_for_task` | `failures_for_task` — `read_history.rs:109` | hand-built object | by hand |
| `memtree_query` | `memtree_query` — `read_history.rs:138` | hand-built object | by hand |
| `memtree_get` | `memtree_get` — `read_history.rs:174` | typed struct | direct |
| `temporal_history` | `temporal_history` — `read_history.rs:187` → `temporal_outcome_json` (`read_history.rs:211`) | hand-built object | by hand |
| `triple_query` | `triple_query` — `read_history.rs:262` | hand-built object | by hand |
| `belief_history` | `belief_history` — `read_history.rs:310` | typed struct | direct |
| `graph_query` | `graph_query` — `read_retrieve.rs:357` | hand-built object | by hand |
| `inspect` | `inspect` — `read_tools.rs:266` | hand-built object | by hand |
| `audit_trail` | `audit_trail` — `read_tools.rs:416` | typed struct | direct |
| `canonical_get` | `canonical_get_tool` — `additive_tools.rs:238` | typed struct | direct |
| `shared_retrieve` | `shared_retrieve` — `additive_tools.rs:289` → `read_retrieve::dispatch` | delegated | by hand |
| `stats` | `stats_tool` — `additive_tools.rs:340` | typed struct | direct |
| `hygiene_audit` | `hygiene_audit_tool` — `hygiene_tools.rs:51` | typed struct | direct |
| `hygiene_log_list` | `hygiene_log_list_tool` — `hygiene_tools.rs:103` | hand-built object | by hand |
| `diagnose` | `diagnose_result` — `ops_tools.rs:122` | hand-built object | partial-base |
| `verify` | `verify_result` — `ops_tools.rs:146` | hand-built object | partial-base |
| `doctor` | `doctor_result` — `ops_tools.rs:179` | hand-built object | partial-base |
| `sync_status` | `sync_status_tool` — `sync_tools.rs:87` | typed struct | direct |
| `sync_key_status` | `sync_key_status_tool` — `sync_tools.rs:160` | hand-built object | by hand |
| `admit_preview_batch` | `preview_batch` — `retention_tools.rs:141` | hand-built object | by hand |
| `retention_profile_get` | `profile_get` — `retention_tools.rs:63` | hand-built object | by hand |
| `summarize` | `summarize` — `summarize_tools.rs:48` → `report` (`summarize_tools.rs:268`) | hand-built object | by hand |
| `config_get` | `dispatch_read` — `config_tools.rs:39` (arm `:43`) | typed struct | direct |
| `config_profiles` | `dispatch_read` — `config_tools.rs:39` (arm `:44`) | typed struct | direct |
| `ranking_env_get` | `dispatch_read` — `config_tools.rs:39` (arm `:45`) | typed struct | direct |

## What a later schema declaration may cover

The 27 `direct` rows are the cheapest. The whole value comes from one typed struct. A declaration could be taken from the type the handler already returns. The 15 `partial-base` rows need their inserted keys described. Each one returns a typed value, and the handler inserts keys after serialization. The 1 `partial-shapes` row needs a union of its shapes. `correct` returns a small `json!` preview on one branch and a typed result on another. The 43 `by hand` rows need a schema written from the assembled keys. No single type describes the object the handler builds. This section only says what a declaration may cover; it does not recommend one, order one, or design one.

## How to re-check

A tool is added or removed in two places. Two files keep this inventory true. One holds the tool name in `crates/clio-types/src/mcp_tools.rs`. The other holds the handler it is routed to, in `crates/clio-mcp/src`. Examples are `write_tools.rs:67`, `read_tools.rs:43` and `additive_tools.rs:39`. To keep a row true, read the handler's return construction. Record the branch that builds the success value. Then classify that value by the definitions in Method.
