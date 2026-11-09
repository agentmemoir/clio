# MCP activation manifest — `2026-07-28`

## Purpose and scope

This file is the activation manifest for issue 544: the stateless MCP production implementation for protocol revision `2026-07-28` is now the only revision this server serves, on one path, with no selector, feature flag, runtime switch, compatibility shim, session fallback, or dual-revision dispatch left anywhere in the tree.

The scope is exactly one revision. Every transport, gate, schema export, config default, test expectation, CI job, benchmark artifact, and document named here speaks `2026-07-28`; the predecessor revision is retained only as a refusal case that fails loudly.

Read this file to audit the change or to revert it. Sections 2 through 6 describe what moved, section 7 is the complete path table and the audit record, section 8 gives the single revert, and sections 9 and 10 record what was verified and what was not.

## Pre-activation state and the activation range

Pre-activation protocol revision: `2025-11-25` (the pinned revision the server advertised before this change).

Pre-activation head (full SHA): `1b9b3e51d7c18e76a9fc0dc1d96363ee00202f95`.

Activation commit range: `1b9b3e5..6cb43cf`. This manifest is the final commit of that range; the four commits below are the activation, in order.

| Commit (full SHA) | Role |
| --- | --- |
| `55723ba4bd6cd73485c08270ff12c1580c936132` | Production wiring: one revision source, candidate modules promoted to production, both transports routed to the stateless dispatcher, and the session and handshake machinery deleted. |
| `b01c0b246135f1bcb9797cd0a20d9c79e0425af0` | Library tests: the `clio-mcp` lib suite rewritten to the stateless wire format. |
| `7a89ec3ccf22b7592273870511b32ffa64c34136` | Integration and conformance: binding harness, conformance suite, and the recorded fixtures aligned to the activated contract. |
| `6cb43cf7b7c2014dd2af26da2fe5eeefc1405628` | CLI, CI, docs, benchmarks, changelog: remaining `crates/clio` expectations swept, golden CLI help regenerated, conformance job renamed, benchmark artifacts and documents moved to the served revision. |

## Single active revision source

The one source is `MCP_PROTOCOL_REVISION`, declared as `pub const MCP_PROTOCOL_REVISION: &str = "2026-07-28";` in `crates/clio-mcp/src/lib.rs`. No other constant, literal, alias, or fixture pin may name a served revision.

Consumers inside `clio-mcp` that now derive from that constant: `crates/clio-mcp/src/stateless.rs` (`SUPPORTED_REVISIONS` is `&[crate::MCP_PROTOCOL_REVISION]`, read by both admission paths and by the `server/discover` result's `supportedVersions`), `crates/clio-mcp/src/protocol.rs` (`McpHandler::protocol_revision()` returns it), `crates/clio-mcp/src/schema.rs` (the schema pack's `mcp_protocol_revision` field), and `crates/clio-mcp/src/stateless_fixtures.rs` (`CANDIDATE_FIXTURE_REVISION` is now the crate pin, so no scenario can sit on a stale revision).

Config and export consumers that carry the same value: `crates/clio-config/src/config/merge.rs` (`mcp.revision` default), `crates/clio-config/src/profile.rs` (profile `mcp_revision` default), `crates/clio-mcp/discovery_tool_defs.json` (the embedded discovery artifact), `crates/clio-mcp/tests/fixtures/schema_pack.json`, `crates/clio-mcp/tests/fixtures/schema_pack_reference.json`, `benchmarks/mcp_token_budget/presets/packs/{all,coding,minimal,ops}.json`, and `benchmarks/mcp_token_budget/baseline/frozen-v1.json`. The two `clio-config` defaults are literals rather than a `use` of the constant because that crate does not depend on `clio-mcp`; `crates/clio-mcp/tests/conformance/revision.rs::revision_pin_is_asserted_everywhere` is what pins them to it, asserting the constant, the schema pack, the served `server/discover` result, and the effective config's `mcp.revision` are the same value, and the fixture-only candidate alias `stateless::CANDIDATE_REVISION` is deleted so no second revision can be named.

## Routing and dispatcher wiring

HTTP: `crates/clio-mcp/src/http.rs` installs `CandidatePost` as the POST strategy in `serve_with_listener` (via `serve_with_listener_and`), and `route` refuses every verb but `POST` through `method_guard::refuse` before the Origin, auth, path, and media-type gates run, so a `GET` is a 405 with no body and there is no server-initiated SSE stream.

HTTP POST strategy: `crates/clio-mcp/src/http_candidate.rs` holds `CandidatePost` and `method_not_found_status` as live code, compiled unconditionally, and `crates/clio-mcp/src/http_post.rs` keeps only the `PostRoute` trait and the request-scoped `PostContext` (`state` and `req`) that the strategy reads. `crates/clio-mcp/src/method_guard.rs` is compiled unconditionally and owns the POST-only verb refusal.

stdio: `crates/clio-mcp/src/stdio.rs`'s `serve(state, input, output)` is the dispatcher itself, not a handler wrapper; `crates/clio/src/mcp_cli.rs` (`clio mcp stdio`) constructs the state and calls it directly. `crates/clio-mcp/src/lib.rs` registers `conditional_http`, `http_candidate`, `method_guard`, and `stateless` as ordinary modules, so nothing on the serving path is test-gated.

## Deletions

Files deleted outright: `crates/clio-mcp/src/http_binding_tests.rs` (the session-and-handshake HTTP binding tests), `crates/clio-mcp/tests/mcp_session_binding.rs` (the integration session-binding suite), and `crates/clio/src/mcp_cli_sessions_tests.rs` (the `--sessions` CLI tests).

Removed production behaviour and symbols, by file: `crates/clio-mcp/src/protocol.rs` lost the `initialize` and `ping` route arms and `McpHandler::initialize`, so both methods now fall to the unknown-method answer (`-32601`; HTTP also returns 404). `crates/clio-mcp/src/http_guard.rs` lost the session store `HttpShared` with its `sessions` map and `session_counter`, `HttpShared::is_live`, `handle_delete` (the `DELETE` termination arm), and `mint_session`, leaving `check_origin` and `origin_host` as the module's whole job. `crates/clio-mcp/src/http.rs` lost `HttpConfig::sessions_enabled` (its field, its default, and the gate that required a live session for any method other than `initialize`), the `MCP-Session-Id` request header read, the `MCP-Session-Id` response header write, the `DELETE` verb arm, and the `HttpShared` plumbing through the accept loop and `route`. `crates/clio-mcp/src/http_post.rs` lost `LivePost` (the session-minting strategy) and the `handler`, `cfg`, `shared`, `identity`, and `request_method` fields of `PostContext`. `crates/clio-mcp/src/stdio.rs` and `crates/clio-mcp/src/http_candidate.rs` lost the test-only `serve_candidate` seams. `crates/clio-mcp/src/stateless.rs` lost the `CANDIDATE_REVISION` alias, and `crates/clio-mcp/src/lib.rs` lost the `#[cfg(test)]` registration of the once-candidate modules `conditional_http`, `http_candidate`, `method_guard`, and `stateless`.

The `--sessions` CLI switch is gone end to end: the `SESSIONS` flag record and its `clio mcp http --bind 127.0.0.1:8765 --sessions` example were removed from `crates/clio/src/cli_command_registry_surface_mcp.rs`, the `sessions_enabled: flags.contains_key("sessions")` read and the `refuse_sessions_value` validation (with its usage hint) and the `sessions_tests` module registration were removed from `crates/clio/src/mcp_cli.rs`, the `[--sessions]` synopsis was removed from `crates/clio/src/cli_help_surfaces.rs`, and the flag name was removed from the expected surface list in `crates/clio/src/cli_flags_test_support.rs`. `docs/mcp-session-state-inventory.md` keeps its historical rows and annotates each removed field as removed.

## Workflow and test registration changes

CI: `.github/workflows/mcp-conformance.yml` renamed the job id from `mcp-conformance-2025-11-25` to `mcp-conformance-2026-07-28`, its display name to `MCP conformance (2026-07-28 pin)`, and its test step to `Run MCP schema, unit, and conformance tests (pinned 2026-07-28)`; the command it runs is unchanged.

Conformance expectations: `crates/clio-mcp/tests/conformance/revision.rs` now asserts the constant, the schema pack, the stdio `server/discover` `supportedVersions`, and the effective config against one value, and the retired handshake is asserted as gone rather than translated. `crates/clio-mcp/tests/conformance/http.rs`, `crates/clio-mcp/tests/conformance/stdio.rs`, and `crates/clio-mcp/tests/conformance/main.rs` build every request from the crate pin with a `params._meta` block plus the `Mcp-Method`, `Mcp-Name`, and `Mcp-Protocol-Version` headers, and a negative case (`t07_http_requires_the_version_header_and_the_served_revision`) sends `Mcp-Protocol-Version: 2025-11-25` and asserts the `-32022` refusal with `requested: "2025-11-25"`.

Fixtures and transcripts: `crates/clio-mcp/tests/fixtures/tools_call_session.json` was recaptured from `target/debug/clio mcp stdio --db sqlite::memory:` as `server/discover` followed by `tools/call`, with no handshake and the capture block recording its source commit and binary digest; `crates/clio-mcp/tests/fixtures/schema_pack.json` and `crates/clio-mcp/tests/fixtures/schema_pack_reference.json` moved only `mcp_protocol_revision`; `crates/clio-mcp/discovery_tool_defs.json` moved the same field.

Golden CLI help: `crates/clio/tests/fixtures/golden_cli_help.json` was regenerated with the repository's own capture mode (`CLIO_GOLDEN_CAPTURE=1` over the golden repeatability test, on a frozen tree), not hand-edited; five entries changed (`help`, `help --json`, `help --help`, `mcp --help`, `mcp http --help`) and the fixture reproduces byte-identically on re-run.

Benchmarks: `benchmarks/mcp_token_budget/presets/packs/{all,coding,minimal,ops}.json` and `benchmarks/mcp_token_budget/baseline/frozen-v1.json` moved to the served revision, `benchmarks/mcp_token_budget/capture_transcript.py` now records one `tools/call` exchange carrying its `_meta` instead of a two-exchange handshake (`EXPECTED_EXCHANGES = 1`, protocol version `2026-07-28`), and `benchmarks/mcp_token_budget/discovery_rows.py` sends the metadata the stateless revision requires.

Remaining CLI-side test registrations were aligned in `crates/clio/src/cli_command_registry_examples_tests.rs`, `crates/clio/src/cli_command_registry_surface_flags_tests.rs`, `crates/clio/src/main_mcp_tests.rs`, `crates/clio/src/main_tests.rs`, `crates/clio/src/mcp_cli_tests.rs`, `crates/clio/src/mcp_cli_tools_tests.rs`, `crates/clio/src/mcp_cli_credentials_tests.rs`, and `crates/clio/src/mcp_cli_insecure_tests.rs`, and the CLI-level MCP traffic in `crates/clio/tests/server_sigpipe_harness.rs`, `crates/clio/tests/cross_path_compliance/review_export_test_support.rs`, and `crates/clio/tests/consolidate_wave_support/mcp_session.rs` now frames stateless requests with `_meta` and the version header.

## Complete path table (audit record)

Source: `git diff --name-status 1b9b3e5..HEAD` at the activation head. Statuses: `A` added, `M` modified, `D` deleted. Totals: 3 added, 78 modified, 3 deleted, 84 paths.

CI and changelog:

| Status | Path |
| --- | --- |
| M | `.github/workflows/mcp-conformance.yml` |
| A | `changelog.d/activate-single-stateless-mcp-revision-issues-544.improvement.md` |

Benchmarks:

| Status | Path |
| --- | --- |
| M | `benchmarks/mcp_token_budget/baseline/frozen-v1.json` |
| M | `benchmarks/mcp_token_budget/capture_transcript.py` |
| M | `benchmarks/mcp_token_budget/discovery_rows.py` |
| M | `benchmarks/mcp_token_budget/presets/packs/all.json` |
| M | `benchmarks/mcp_token_budget/presets/packs/coding.json` |
| M | `benchmarks/mcp_token_budget/presets/packs/minimal.json` |
| M | `benchmarks/mcp_token_budget/presets/packs/ops.json` |

Config crate:

| Status | Path |
| --- | --- |
| M | `crates/clio-config/src/config/merge.rs` |
| M | `crates/clio-config/src/profile.rs` |

`clio-mcp` crate files and modules:

| Status | Path |
| --- | --- |
| M | `crates/clio-mcp/README.md` |
| M | `crates/clio-mcp/discovery_tool_defs.json` |
| M | `crates/clio-mcp/src/conditional_http.rs` |
| M | `crates/clio-mcp/src/conditional_http_event_id_tests.rs` |
| M | `crates/clio-mcp/src/conditional_http_tests.rs` |
| M | `crates/clio-mcp/src/conn_pool_transport_bounds_tests.rs` |
| M | `crates/clio-mcp/src/discovery_tool_defs_tests.rs` |
| M | `crates/clio-mcp/src/http.rs` |
| D | `crates/clio-mcp/src/http_binding_tests.rs` |
| M | `crates/clio-mcp/src/http_candidate.rs` |
| M | `crates/clio-mcp/src/http_candidate_fixtures.rs` |
| M | `crates/clio-mcp/src/http_candidate_tests.rs` |
| M | `crates/clio-mcp/src/http_guard.rs` |
| M | `crates/clio-mcp/src/http_guard_tests.rs` |
| M | `crates/clio-mcp/src/http_post.rs` |
| M | `crates/clio-mcp/src/http_post_tests.rs` |
| M | `crates/clio-mcp/src/http_request_bounds_tests.rs` |
| A | `crates/clio-mcp/src/http_retention_parity_tests.rs` |
| M | `crates/clio-mcp/src/http_start_auth_tests.rs` |
| M | `crates/clio-mcp/src/http_test_support_tests.rs` |
| M | `crates/clio-mcp/src/http_tests.rs` |
| M | `crates/clio-mcp/src/lib.rs` |
| M | `crates/clio-mcp/src/method_guard.rs` |
| M | `crates/clio-mcp/src/method_guard_tests.rs` |
| M | `crates/clio-mcp/src/principal.rs` |
| M | `crates/clio-mcp/src/principal_tests.rs` |
| M | `crates/clio-mcp/src/protocol.rs` |
| M | `crates/clio-mcp/src/protocol_tests.rs` |
| M | `crates/clio-mcp/src/read_retrieve_dedup_tests.rs` |
| M | `crates/clio-mcp/src/schema_retention_defs_tests.rs` |
| M | `crates/clio-mcp/src/schema_tests.rs` |
| M | `crates/clio-mcp/src/stateless.rs` |
| M | `crates/clio-mcp/src/stateless_cache_tests.rs` |
| M | `crates/clio-mcp/src/stateless_fixtures.rs` |
| M | `crates/clio-mcp/src/stateless_tests.rs` |
| M | `crates/clio-mcp/src/stdio.rs` |
| M | `crates/clio-mcp/src/stdio_candidate_listing_tests.rs` |
| M | `crates/clio-mcp/src/stdio_candidate_tests.rs` |
| M | `crates/clio-mcp/src/stdio_tests.rs` |
| M | `crates/clio-mcp/src/version_header_tests.rs` |

`clio-mcp` integration, conformance, and fixtures:

| Status | Path |
| --- | --- |
| M | `crates/clio-mcp/tests/conformance/http.rs` |
| M | `crates/clio-mcp/tests/conformance/main.rs` |
| M | `crates/clio-mcp/tests/conformance/revision.rs` |
| M | `crates/clio-mcp/tests/conformance/stdio.rs` |
| M | `crates/clio-mcp/tests/fixtures/schema_pack.json` |
| M | `crates/clio-mcp/tests/fixtures/schema_pack_reference.json` |
| M | `crates/clio-mcp/tests/fixtures/tools_call_session.json` |
| M | `crates/clio-mcp/tests/mcp_audit_conformance.rs` |
| M | `crates/clio-mcp/tests/mcp_principal_binding.rs` |
| M | `crates/clio-mcp/tests/mcp_principal_binding_support/mod.rs` |
| A | `crates/clio-mcp/tests/mcp_principal_binding_support/wire.rs` |
| M | `crates/clio-mcp/tests/mcp_read_mock.rs` |
| M | `crates/clio-mcp/tests/mcp_read_transport_conformance.rs` |
| D | `crates/clio-mcp/tests/mcp_session_binding.rs` |
| M | `crates/clio-mcp/tests/tools_call_session/main.rs` |

`clio` crate and its tests and fixtures:

| Status | Path |
| --- | --- |
| M | `crates/clio/src/cli_command_registry_examples_tests.rs` |
| M | `crates/clio/src/cli_command_registry_surface_flags_tests.rs` |
| M | `crates/clio/src/cli_command_registry_surface_mcp.rs` |
| M | `crates/clio/src/cli_flags_test_support.rs` |
| M | `crates/clio/src/cli_help_surfaces.rs` |
| M | `crates/clio/src/main_mcp_tests.rs` |
| M | `crates/clio/src/main_tests.rs` |
| M | `crates/clio/src/mcp_cli.rs` |
| M | `crates/clio/src/mcp_cli_credentials_tests.rs` |
| M | `crates/clio/src/mcp_cli_insecure_tests.rs` |
| D | `crates/clio/src/mcp_cli_sessions_tests.rs` |
| M | `crates/clio/src/mcp_cli_tests.rs` |
| M | `crates/clio/src/mcp_cli_tools_tests.rs` |
| M | `crates/clio/tests/consolidate_wave_support/mcp_session.rs` |
| M | `crates/clio/tests/cross_path_compliance/review_export_test_support.rs` |
| M | `crates/clio/tests/fixtures/golden_cli_help.json` |
| M | `crates/clio/tests/server_sigpipe_harness.rs` |

Documentation:

| Status | Path |
| --- | --- |
| M | `docs/mcp-session-state-inventory.md` |

This manifest, `docs/mcp-activation-2026-07-28.md`, is untracked when read locally and is added by the activation's final commit, so it is not part of the table above.

## Revert

The whole activation reverts in one command: `git revert --no-edit 1b9b3e5..6cb43cf`. After the pull request is merged, reverting the single merge commit reverts the same content. This manifest is added by the activation and is removed by the same revert.

Path-level fallback, using the table above as the work list: restore every `M` path and every `D` path from the pre-activation head with `git checkout 1b9b3e5 -- <path>` (that also re-creates the three deleted files), and delete every `A` path with `git rm -f <path>` — `.github/workflows/mcp-conformance.yml` is the only modified path outside `crates/` and `docs/`, and this manifest is deleted as an added path.

## Verification run on the activation head

Toolchain check: `cargo check --locked -p clio-mcp -p clio-config -p clio --all-targets` exits 0 with no warnings.

Library suite: `cargo test -p clio-mcp --lib` reports `1220 passed; 0 failed` under a fresh `XDG_DATA_HOME`; the same target on the ambient host state reports the subject-request and sync cases as failing because the host erasure journal was recorded under a different deployment salt, and they pass under the fresh directory.

Integration and conformance: `cargo test -p clio-mcp --tests` is green for every edited target (27 binaries, 145 test functions, run with `--no-fail-fast`); the two Postgres subject-request binaries fail on the ambient host state only, and both pass (`2 passed` each) under the fresh `XDG_DATA_HOME`.

CLI suites: `cargo test -p clio --lib` reports `3 passed; 0 failed` and `cargo test -p clio --bins` reports `2803 passed; 0 failed`; `cargo check -p clio --all-targets` exits 0.

Golden help repeatability: `CLIO_GOLDEN_CAPTURE=1` over the golden repeatability test (`capture_mode_writes_the_fixture_only_when_asked`) passes, and a second capture on the frozen tree reproduces `crates/clio/tests/fixtures/golden_cli_help.json` byte-identically; `golden_repeatability_test` plus `server_sigpipe_harness` run as 8 tests, all passing.

Changelog fragment: `python3 scripts/changelog_fragments.py --check` reports the fragments valid (exit 0), including `changelog.d/activate-single-stateless-mcp-revision-issues-544.improvement.md`.

Repository hooks: `make precommit` exits 0 with all nine hooks passing, the CLI-reference hook reporting 0 findings against the rebuilt binary.

Anything not listed above was not run locally, and its verdict belongs to CI.

## Known limits

The Postgres-backed consolidate-wave and cross-path compliance suites (`crates/clio/tests/consolidate_wave_support/`, `crates/clio/tests/cross_path_compliance/`) were not executed locally beyond the edited request-framing helpers, so their full verdict is CI's.

CI owns the complete test and coverage verdict. This manifest records only the commands and results above and marks everything else as CI's to decide.
