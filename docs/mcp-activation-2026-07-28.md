# MCP activation manifest — `2026-07-28`

## Purpose and scope

This file is the activation manifest for issue 544: the stateless MCP production implementation for protocol revision `2026-07-28` is now the only revision this server serves, on one path, with no selector, feature flag, runtime switch, compatibility shim, session fallback, or dual-revision dispatch left anywhere in the tree.

The scope is exactly one revision. Every transport, gate, schema export, config default, test expectation, CI job, benchmark artifact, and document named here speaks `2026-07-28`; the predecessor revision is retained only as a refusal case that fails loudly.

Read this file to audit the change or to revert it. The sections below describe what moved, give the complete path table and how it was checked, state the one revert command and the dry run that proves it, and record what was verified and what is CI's.

## Pre-activation state and the activation range

Pre-activation protocol revision: `2025-11-25` (the pinned revision the server advertised before this change).

Pre-activation head (full SHA): `1b9b3e51d7c18e76a9fc0dc1d96363ee00202f95`.

Activation commit range: `1b9b3e5..4c5681e` — seven commits, listed in order below. `HEAD` in this range is `4c5681e`; the branch is `issue/544-activate-the-complete`.

| Commit (full SHA) | Role |
| --- | --- |
| `55723ba4bd6cd73485c08270ff12c1580c936132` | Production wiring: one revision source, candidate modules promoted to production, both transports routed to the stateless dispatcher, and the session and handshake machinery deleted. |
| `b01c0b246135f1bcb9797cd0a20d9c79e0425af0` | Library tests: the `clio-mcp` lib suite rewritten to the stateless wire format. |
| `7a89ec3ccf22b7592273870511b32ffa64c34136` | Integration and conformance: binding harness, conformance suite, and the recorded fixtures aligned to the activated contract. |
| `6cb43cf7b7c2014dd2af26da2fe5eeefc1405628` | CLI, CI, docs, benchmarks, changelog: remaining `crates/clio` expectations swept, golden CLI help regenerated, conformance job renamed, benchmark artifacts and documents moved to the served revision. |
| `cf34aa9cab6f8fdfdf2675e4cbff0b0ac83276ae` | Documentation: the first version of this manifest (declared range and revert command corrected by the rewrite below). |
| `5e872d24059f7014a9cb68f226f623a9aa495683` | CI repair after activation: `cargo fmt` drift in five `clio-mcp` test files and the conformance pair fixed, and the five benchmark digest pins re-pointed at the new baseline record. |
| `4c5681e682ea9bc805dfcf9e5ec3cc8128c05ac9` | Review fix: a removed method (`initialize`, `ping`) is answered `-32601` before the version-header and metadata gates, in `admit` and `dispatch` from one helper, with a test on each transport. |

This manifest rewrite itself is the next commit — the eighth — and it is the only file it modifies: `docs/mcp-activation-2026-07-28.md`. The path table below is `git diff --name-status 1b9b3e5..4c5681e`, so every status is measured against the pre-activation base, and a commit that only edits a file whose base status is already `A` leaves both the path set and every status in the table exact after the rewrite lands. The line counts in `git diff --shortstat 1b9b3e5..4c5681e` (92 files, 1676 insertions, 2058 deletions) likewise move only by this file's own delta.

## Single active revision source

The one source is `MCP_PROTOCOL_REVISION`, declared as `pub const MCP_PROTOCOL_REVISION: &str = "2026-07-28";` in `crates/clio-mcp/src/lib.rs:127`. No other constant, literal, alias, or fixture pin may name a served revision.

Consumers inside `clio-mcp` that derive from that constant: `crates/clio-mcp/src/stateless.rs:49` (`SUPPORTED_REVISIONS` is `&[crate::MCP_PROTOCOL_REVISION]`, read by both admission paths and by the `server/discover` result's `supportedVersions`), `crates/clio-mcp/src/protocol.rs:25` and `:66` (`McpHandler::protocol_revision()` returns it), `crates/clio-mcp/src/schema.rs:25` and `:125` (the schema pack's `mcp_protocol_revision` field), and `crates/clio-mcp/src/stateless_fixtures.rs:47` (`CANDIDATE_FIXTURE_REVISION` is the crate pin, so no scenario can sit on a stale revision).

Two more sites alias the constant rather than restating it, and both are unchanged by the activation: `crates/clio-mcp/src/runtime.rs:56` declares `pub const PROTOCOL_REVISION: &str = crate::MCP_PROTOCOL_REVISION;` for diagnostics assertions, and `crates/clio-mcp/src/tools_page.rs:123` falls back to `crate::MCP_PROTOCOL_REVISION.to_owned()` for the `ToolView` revision when the caller names none.

Config and export consumers that carry the same value: `crates/clio-config/src/config/merge.rs` (`mcp.revision` default), `crates/clio-config/src/profile.rs` (profile `mcp_revision` default), `crates/clio-mcp/discovery_tool_defs.json` (the embedded discovery artifact), `crates/clio-mcp/tests/fixtures/schema_pack.json`, `crates/clio-mcp/tests/fixtures/schema_pack_reference.json`, `benchmarks/mcp_token_budget/presets/packs/{all,coding,minimal,ops}.json`, and `benchmarks/mcp_token_budget/baseline/frozen-v1.json`. The two `clio-config` defaults are literals rather than a `use` of the constant because that crate does not depend on `clio-mcp`; `crates/clio-mcp/tests/conformance/revision.rs::revision_pin_is_asserted_everywhere` is what pins them to it, asserting the constant, the schema pack, the served `server/discover` result, and the effective config's `mcp.revision` are the same value, and the fixture-only candidate alias `stateless::CANDIDATE_REVISION` is deleted so no second revision can be named.

## Routing and dispatcher wiring

HTTP: `crates/clio-mcp/src/http.rs` installs `CandidatePost` as the POST strategy in `serve_with_listener` (via `serve_with_listener_and`), and `route` refuses every verb but `POST` through `method_guard::refuse` before the Origin, auth, path, and media-type gates run, so a `GET` is a 405 with no body, and a POST `subscriptions/listen` request answers with a `text/event-stream` body.

HTTP POST strategy: `crates/clio-mcp/src/http_candidate.rs` holds `CandidatePost` and `method_not_found_status` as live code, compiled unconditionally, and `crates/clio-mcp/src/http_post.rs` keeps only the `PostRoute` trait and the request-scoped `PostContext` (`state` and `req`) that the strategy reads. `crates/clio-mcp/src/method_guard.rs` is compiled unconditionally and owns the POST-only verb refusal.

stdio: `crates/clio-mcp/src/stdio.rs`'s `serve(state, input, output)` is the dispatcher itself, not a handler wrapper; `crates/clio/src/mcp_cli.rs` (`clio mcp stdio`) constructs the state and calls it directly. `crates/clio-mcp/src/lib.rs` registers `conditional_http`, `http_candidate`, `method_guard`, and `stateless` as ordinary modules, so nothing on the serving path is test-gated.

## Deletions

Files deleted outright: `crates/clio-mcp/src/http_binding_tests.rs` (the session-and-handshake HTTP binding tests), `crates/clio-mcp/tests/mcp_session_binding.rs` (the integration session-binding suite), and `crates/clio/src/mcp_cli_sessions_tests.rs` (the `--sessions` CLI tests).

Removed production behaviour and symbols, by file: `crates/clio-mcp/src/protocol.rs` lost the `initialize` and `ping` route arms and `McpHandler::initialize`, so both methods now fall to the unknown-method answer (`-32601`; HTTP also returns 404). `crates/clio-mcp/src/http_guard.rs` lost the session store `HttpShared` with its `sessions` map and `session_counter`, `HttpShared::is_live`, `handle_delete` (the `DELETE` termination arm), and `mint_session`, leaving `check_origin` and `origin_host` as the module's whole job. `crates/clio-mcp/src/http.rs` lost `HttpConfig::sessions_enabled` (its field, its default, and the gate that required a live session for any method other than `initialize`), the `MCP-Session-Id` request header read, the `MCP-Session-Id` response header write, the `DELETE` verb arm, and the `HttpShared` plumbing through the accept loop and `route`. `crates/clio-mcp/src/http_post.rs` lost `LivePost` (the session-minting strategy) and the `handler`, `cfg`, `shared`, `identity`, and `request_method` fields of `PostContext`. `crates/clio-mcp/src/stdio.rs` and `crates/clio-mcp/src/http_candidate.rs` lost the test-only `serve_candidate` seams. `crates/clio-mcp/src/stateless.rs` lost the `CANDIDATE_REVISION` alias, and `crates/clio-mcp/src/lib.rs` lost the `#[cfg(test)]` registration of the once-candidate modules `conditional_http`, `http_candidate`, `method_guard`, and `stateless`.

Refusal ordering after the review fix (`4c5681e`): `crates/clio-mcp/src/stateless.rs` answers a removed method from one helper, reading `REMOVED_METHODS` and requiring an `id`, and returns it from `admit` before the version-header read and from `dispatch` before the metadata rule, so a bare `initialize` or `ping` is `-32601` on both transports — 404 on HTTP through `http_candidate::method_not_found_status`, one error line on stdio — and the HTTP admission and stdio cannot drift. The rule is covered by `stateless::version_header_tests::a_removed_method_is_answered_before_the_version_header_and_the_metadata`, `http_candidate::http_candidate_guard_tests::http_bare_removed_methods_answer_not_found_before_the_metadata_rule`, and the new file `crates/clio-mcp/src/stdio_removed_method_tests.rs` (`stdio::stdio_removed_method_tests`). The admission-order prose in `admit`'s doc comment and in `version_header_tests`' module doc was corrected in the same commit.

The `--sessions` CLI switch is gone end to end: the `SESSIONS` flag record and its `clio mcp http --bind 127.0.0.1:8765 --sessions` example were removed from `crates/clio/src/cli_command_registry_surface_mcp.rs`, the `sessions_enabled: flags.contains_key("sessions")` read and the `refuse_sessions_value` validation (with its usage hint) and the `sessions_tests` module registration were removed from `crates/clio/src/mcp_cli.rs`, the `[--sessions]` synopsis was removed from `crates/clio/src/cli_help_surfaces.rs`, and the flag name was removed from the expected surface list in `crates/clio/src/cli_flags_test_support.rs`. `docs/mcp-session-state-inventory.md` keeps its historical rows and annotates each removed field as removed.

## Workflow, fixtures, and benchmark changes

CI: `.github/workflows/mcp-conformance.yml` renamed the job id from `mcp-conformance-2025-11-25` to `mcp-conformance-2026-07-28`, its display name to `MCP conformance (2026-07-28 pin)`, and its test step to `Run MCP schema, unit, and conformance tests (pinned 2026-07-28)`; the command it runs is unchanged.

Conformance expectations: `crates/clio-mcp/tests/conformance/revision.rs` now asserts the constant, the schema pack, the stdio `server/discover` `supportedVersions`, and the effective config against one value, and the retired handshake is asserted as gone rather than translated. `crates/clio-mcp/tests/conformance/http.rs`, `crates/clio-mcp/tests/conformance/stdio.rs`, and `crates/clio-mcp/tests/conformance/main.rs` build every request from the crate pin with a `params._meta` block plus the `Mcp-Method`, `Mcp-Name`, and `Mcp-Protocol-Version` headers, and a negative case (`t07_http_requires_the_version_header_and_the_served_revision`) sends `Mcp-Protocol-Version: 2025-11-25` and asserts the `-32022` refusal with `requested: "2025-11-25"`.

Fixtures and transcripts: `crates/clio-mcp/tests/fixtures/tools_call_session.json` was recaptured from `target/debug/clio mcp stdio --db sqlite::memory:` as `server/discover` followed by `tools/call`, with no handshake and the capture block recording its source commit and binary digest; `crates/clio-mcp/tests/fixtures/schema_pack.json` and `crates/clio-mcp/tests/fixtures/schema_pack_reference.json` moved only `mcp_protocol_revision`; `crates/clio-mcp/discovery_tool_defs.json` moved the same field.

Golden CLI help: `crates/clio/tests/fixtures/golden_cli_help.json` was regenerated with the repository's own capture mode (`CLIO_GOLDEN_CAPTURE=1` over the golden repeatability test, on a frozen tree), not hand-edited; five entries changed (`help`, `help --json`, `help --help`, `mcp --help`, `mcp http --help`) and the fixture reproduces byte-identically on re-run.

Benchmarks and the baseline digest: `benchmarks/mcp_token_budget/presets/packs/{all,coding,minimal,ops}.json` and `benchmarks/mcp_token_budget/baseline/frozen-v1.json` moved to the served revision, `benchmarks/mcp_token_budget/capture_transcript.py` now records one `tools/call` exchange carrying its `_meta` instead of a two-exchange handshake (`EXPECTED_EXCHANGES = 1`, protocol version `2026-07-28`), and `benchmarks/mcp_token_budget/discovery_rows.py` sends the metadata the stateless revision requires.

The baseline record carries a digest that five files pin: `benchmarks/mcp_token_budget/allowances/frozen-v1.json`, `benchmarks/mcp_token_budget/targets/frozen-v1.json`, `benchmarks/mcp_token_budget/presets/frozen-v1.json`, `benchmarks/mcp_token_budget/budget_targets.py` (`PINNED_BASELINE_SHA256`), and `benchmarks/mcp_token_budget/test_baseline.py` (`FROZEN_V1_SHA256`). All six files moved together in `5e872d2` to `90e36e7224645e3d6584fb7834b0f95b0f1d275f50a41fa60444f399e7ca62a2`, so any revert or amendment must move the baseline record and all five pins as one unit or `scripts/mcp_budget_check.sh` and `python3 -m unittest discover -s benchmarks/mcp_token_budget` stop agreeing with the tree.

Remaining CLI-side test registrations were aligned in `crates/clio/src/cli_command_registry_examples_tests.rs`, `crates/clio/src/cli_command_registry_surface_flags_tests.rs`, `crates/clio/src/main_mcp_tests.rs`, `crates/clio/src/main_tests.rs`, `crates/clio/src/mcp_cli_tests.rs`, `crates/clio/src/mcp_cli_tools_tests.rs`, `crates/clio/src/mcp_cli_credentials_tests.rs`, and `crates/clio/src/mcp_cli_insecure_tests.rs`, and the CLI-level MCP traffic in `crates/clio/tests/server_sigpipe_harness.rs`, `crates/clio/tests/cross_path_compliance/review_export_test_support.rs`, and `crates/clio/tests/consolidate_wave_support/mcp_session.rs` now frames stateless requests with `_meta` and the version header.

## Complete path table (audit record)

Source: `git diff --name-status 1b9b3e5..4c5681e`. Statuses: `A` added, `M` modified, `D` deleted. Totals: 84 modified, 5 added, 3 deleted — 92 paths.

The five added paths are `changelog.d/activate-single-stateless-mcp-revision-issues-544.improvement.md`, `crates/clio-mcp/src/http_retention_parity_tests.rs`, `crates/clio-mcp/src/stdio_removed_method_tests.rs`, `crates/clio-mcp/tests/mcp_principal_binding_support/wire.rs`, and this manifest `docs/mcp-activation-2026-07-28.md`. The three deleted paths are `crates/clio-mcp/src/http_binding_tests.rs`, `crates/clio-mcp/tests/mcp_session_binding.rs`, and `crates/clio/src/mcp_cli_sessions_tests.rs`.

### CI and changelog

| Status | Path |
| --- | --- |
| M | `.github/workflows/mcp-conformance.yml` |
| A | `changelog.d/activate-single-stateless-mcp-revision-issues-544.improvement.md` |

1 added, 1 modified, 0 deleted — 2 paths.

### Benchmarks (`benchmarks/mcp_token_budget`)

| Status | Path |
| --- | --- |
| M | `benchmarks/mcp_token_budget/allowances/frozen-v1.json` |
| M | `benchmarks/mcp_token_budget/baseline/frozen-v1.json` |
| M | `benchmarks/mcp_token_budget/budget_targets.py` |
| M | `benchmarks/mcp_token_budget/capture_transcript.py` |
| M | `benchmarks/mcp_token_budget/discovery_rows.py` |
| M | `benchmarks/mcp_token_budget/presets/frozen-v1.json` |
| M | `benchmarks/mcp_token_budget/presets/packs/all.json` |
| M | `benchmarks/mcp_token_budget/presets/packs/coding.json` |
| M | `benchmarks/mcp_token_budget/presets/packs/minimal.json` |
| M | `benchmarks/mcp_token_budget/presets/packs/ops.json` |
| M | `benchmarks/mcp_token_budget/targets/frozen-v1.json` |
| M | `benchmarks/mcp_token_budget/test_baseline.py` |

0 added, 12 modified, 0 deleted — 12 paths.

### Config crate (`clio-config`)

| Status | Path |
| --- | --- |
| M | `crates/clio-config/src/config/merge.rs` |
| M | `crates/clio-config/src/profile.rs` |

0 added, 2 modified, 0 deleted — 2 paths.

### `clio-mcp` crate: crate root, modules, and fixtures

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
| M | `crates/clio-mcp/src/http_candidate_guard_tests.rs` |
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
| A | `crates/clio-mcp/src/stdio_removed_method_tests.rs` |
| M | `crates/clio-mcp/src/stdio_tests.rs` |
| M | `crates/clio-mcp/src/version_header_tests.rs` |

2 added, 39 modified, 1 deleted — 42 paths.

### `clio-mcp` crate: integration, conformance, and support

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

1 added, 13 modified, 1 deleted — 15 paths.

### `clio` crate: sources, tests, and fixtures

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

0 added, 16 modified, 1 deleted — 17 paths.

### Documentation

| Status | Path |
| --- | --- |
| A | `docs/mcp-activation-2026-07-28.md` |
| M | `docs/mcp-session-state-inventory.md` |

1 added, 1 modified, 0 deleted — 2 paths.

### How the table was checked

`git diff --name-status 1b9b3e5..4c5681e | wc -l` reports 92, and its status histogram is `84 M`, `5 A`, `3 D`; the tables above were generated from that exact command's output, and a script re-reads this file and compares the (status, path) pairs it lists against the command's pairs as sets — 92 of 92 match, with nothing extra on either side. The group counts add up: 2 + 12 + 2 + 42 + 15 + 17 + 2 = 92.

## Revert

The whole activation reverts in one command from the merged branch: `git revert --no-edit 1b9b3e5..HEAD`, where `HEAD` is the branch head at merge time — `4c5681e` plus this rewrite, the eighth commit — so the range covers all seven activation commits and the manifest commit. It produces one revert commit per commit in the range, in reverse order. After the pull request is merged, reverting the single merge commit (`git revert --no-edit -m 1 <merge-sha>`) reverts the same content, and this manifest is deleted by both because it is an added path.

Dry run of that exact command, in a throwaway clone made with `git clone --shared --no-hardlinks . <scratch>/revert-dryrun` (it shares this repository's object store but is not a worktree of it), checked out at `4c5681e`: the clone ran `git revert --no-edit 1b9b3e5..HEAD`, which exited 0 with no conflict and no manual step and produced seven revert commits on top of `4c5681e`; `git status --porcelain` was empty afterwards (clean tree) and `git diff 1b9b3e5..HEAD` in the clone was empty, so the reverted tree is byte-identical to the pre-activation base. Run against a binary built from that reverted tree, `scripts/mcp_budget_check.sh` exited 0 with verdict `pass` and no breaches or failures, and `baseline/frozen-v1.json` plus all five digest pins agreed on `484b46ac9ee2703920ee7f21f7f1eb16a00feca36336269489bf10e718e3be40`, which is the pre-activation digest. `python3 -m unittest discover -s benchmarks/mcp_token_budget` in the same clone ran 319 tests with 1 failure and 1 error (2 skipped): `test_offline_run_succeeds` cannot import the tokenizer once the test replaces `HOME`, and `test_script_reports_export_failure_cleanly` cannot execute the shell script on the no-exec scratch filesystem — both reproduce identically in a control clone of the unreverted head under the same conditions, so they are limits of the throwaway location and not of the revert. Repeating the procedure with this rewrite committed as the eighth commit — a one-file commit on top of `4c5681e` — also reverted cleanly: exit 0, a clean tree, and a tree equal to the pre-activation base, which is the state the merged branch will be in.

Path-level fallback, using the tables above as the work list: restore every `M` path and every `D` path from the pre-activation head with `git checkout 1b9b3e5 -- <path>` (that also re-creates the three deleted files), and delete every `A` path with `git rm -f <path>` — that includes this manifest, `docs/mcp-activation-2026-07-28.md`. The list must be taken from all seven tables, not from the pre-`5e872d2` subset: `benchmarks/mcp_token_budget/allowances/frozen-v1.json`, `targets/frozen-v1.json`, `presets/frozen-v1.json`, `budget_targets.py`, and `test_baseline.py` all pin the digest of `baseline/frozen-v1.json`, so restoring the baseline without restoring the five pins leaves them inconsistent with the restored record and the budget check fails. Restoring all six together, which is what the one-command revert does, keeps them consistent.

## Verification

Every row below names the head it was measured at. `HEAD` means the current tree (`4c5681e` with this rewrite applied, which changes no code and no test).

| Command | Head | Result |
| --- | --- | --- |
| `cargo test --locked -p clio-mcp --lib` with a fresh `XDG_DATA_HOME` and `CLIO_DATABASE_URL` at the `Makefile` default | rewrite | exit 0, `1223 passed; 0 failed; 0 ignored` |
| `cargo test -p clio-mcp --tests --no-fail-fast` | `4c5681e` | 28 result lines, every one `0 failed`; 1368 passed, 1 pre-existing ignored |
| `cargo fmt --all -- --check` | rewrite | exit 0 |
| `scripts/mcp_budget_check.sh` in the reverted clone | reverted tree | exit 0, verdict `pass` |
| `python3 -m unittest discover -s benchmarks/mcp_token_budget` in the reverted clone | reverted tree | 319 tests, 1 failure and 1 error, both reproduced identically in a control clone of the unreverted head (test runner environment, not the revert) |
| `cargo check --locked -p clio-mcp -p clio-config -p clio --all-targets` | `6cb43cf` | exit 0, no warnings |
| `cargo test -p clio --lib` (`3 passed; 0 failed`) and `cargo test -p clio --bins` (`2803 passed; 0 failed`) | `6cb43cf` | exit 0 |
| `CLIO_GOLDEN_CAPTURE=1` over the golden repeatability test, second capture byte-identical; `golden_repeatability_test` plus `server_sigpipe_harness` as 8 tests | `6cb43cf` | pass |
| `python3 scripts/changelog_fragments.py --check` | `6cb43cf` | exit 0 |
| `make precommit` | `4c5681e` | exit 0, nine hooks over the tracked tree, CLI-reference hook 0 findings |

The two Postgres-facing subject-request binaries fail on the ambient host state only, because the host erasure journal was recorded under a different deployment salt; they pass (`2 passed` each) under the fresh `XDG_DATA_HOME` used for the row above, and the numbers recorded here are from that fresh environment.

Anything not listed above was not run locally, and its verdict belongs to CI: CI is the full test, coverage, and budget verdict for this branch, and it runs the same `scripts/mcp_budget_check.sh` the row above names.

## Known limits

The Postgres-backed consolidate-wave and cross-path compliance suites (`crates/clio/tests/consolidate_wave_support/`, `crates/clio/tests/cross_path_compliance/`) were not executed locally beyond the edited request-framing helpers, so their full verdict is CI's.

This manifest records the commands and results above and nothing more; every other suite, and the final green verdict on the merged branch, is CI's to decide.
