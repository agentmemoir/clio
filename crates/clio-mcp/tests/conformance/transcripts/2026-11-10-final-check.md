# Final check record for build 7096d35

## Scope and placement

This record joins the existing feature and continuous-integration check receipts for one finished build of the MCP activation work. It is evidence, not reader-facing reference, so it lives with the MCP conformance transcripts. No test, script, or workflow reads this directory, so the record is inert. The record verifies freshness and completeness; it does not rerun a category's checks. Its only new execution is one bounded final smoke.

## Build and source identity

| Field | Value |
| --- | --- |
| Build head | `7096d353e564f1287aab3c27ca7d9567192f62e5` |
| Build tree | `0de0469025941d46e43199f17544d15c9f22014a` |
| Source branch | `master` |
| Base | `master` at the same commit |
| Merge | PR #607, which merged the issue 546 work |
| Date | 2026-11-10 |

The build identity is the commit and tree above; evidence from any other head or tree is stale for this build unless the relevant source paths are byte-identical, and each category row states which case holds.

## CI at the build head

Five workflows ran on the build head, and all five completed successfully.

| Workflow | Run | Conclusion |
| --- | --- | --- |
| Benchmarks | https://github.com/agentmemoir/clio/actions/runs/42778397509 | success |
| First Defence | https://github.com/agentmemoir/clio/actions/runs/42778397508 | success |
| CodeQL | https://github.com/agentmemoir/clio/actions/runs/42778397481 | success |
| CI | https://github.com/agentmemoir/clio/actions/runs/42778397471 | success |
| MCP conformance | https://github.com/agentmemoir/clio/actions/runs/42778397551 | success |

The other workflows in the repository were either path-filtered out of this change or triggered by a different event, so their absence is not a pass, and a run URL is a pointer to the producing output, not the output itself.

## Freshness and staleness

A receipt is fresh for this build only when its build identity is this head or its relevant source paths are byte-identical to this tree.
Stale evidence is rejected and recorded as `missing`, never as `pass`.
A row that cannot be produced is recorded as `missing` with its cause, and never estimated.

## Category receipts

One fact governs every row below: at this build head CI runs Rust tests in exactly one job, and that job is the MCP conformance job running `cargo test -p clio-mcp --locked`. The scoped-coverage job resolved its base to HEAD~1 and selected zero changed Rust files and zero test selections, so no other crate's tests ran at this head.

The table below follows the contract: each row names its producing receipt, the output or CI link, the build and source identity, the pass/fail/missing state, the provenance label, and the staleness note. A row is never marked passing merely because a source file was read.

| Category | Producing receipt | Output or CI link | Build identity | Source identity | State | Provenance label | Staleness note |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Measurement and budgets | PR #597 (preset targets), PR #593 (activated skill body row), PR #578 (discovery cost record) | https://github.com/agentmemoir/clio/actions/runs/42778397509/job/129702235185 | 7096d35 (tree 0de0469); cl100k_base, tiktoken 0.14.0, asset sha256 `223921b7...65b2a7` | 7096d35; `benchmarks/mcp_token_budget/baseline/frozen-v1.json` | pass | Measured | Same-head job: verdict pass, 8 of 8 comparisons pass, `activated_skill_body/tokens` ceiling 594 passes. The always-loaded entry-point arm is `not-measured` at this head, so that arm carries frozen rows only. |
| Catalog and discovery safety | PR #573 (deterministic catalog search), PR #575 (describe any catalog tool), PR #578 | https://github.com/agentmemoir/clio/actions/runs/42778397551/job/129702235248 | 7096d35 (tree 0de0469) | 7096d35 | pass | Measured | Same-head suite: the lib run includes `catalog_search` (22), `catalog_invoke` (20) and `discovery_tools` (10) tests, all green. |
| Schema and result shapes (inventory, one payload, local reference closure, batch reuse) | No merged feature change produced a scoped receipt; the same-head CI job is the producing receipt | https://github.com/agentmemoir/clio/actions/runs/42778397551/job/129702235248 | 7096d35 (tree 0de0469) | 7096d35 | pass | Measured | `schema_pack_snapshot` 2, `pack_shape_record` 5 and `tools_call_session` 2 tests pass. The client shared-reference probe stays `not_run` in `pack_shape.json`, so that probe is Unverified and the flattened pack ships. |
| Paging and subscriptions | PR #570 (cursor refusals), PR #604 (server-owned list-change stream) | https://github.com/agentmemoir/clio/actions/runs/42778397551/job/129702235248 | 7096d35 (tree 0de0469) | 7096d35 | pass | Measured | Same-head lib run covers `tools_page` (18), `tool_listen` (16), `tool_snapshot` (3) and `event_stream` (2) tests; the HTTP candidate subscription tests compile and run in that job. |
| Catalog hash mutation and revert; skill regeneration policy | PR #586 (skill ownership and catalog-hash drift) | https://github.com/agentmemoir/clio/actions/runs/42778397551/job/129702235248 | 7096d35 (tree 0de0469) | 7096d35 | pass | Measured | At this head `tests/skill_drift/main.rs` is 30 passed and 1 ignored; the ignored test is the regenerator that writes `catalog_hash.txt`. `tests/skill_drift/catalog_hash.rs` is byte-identical to PR #586's head, so that mutation and revert run still binds; the non-ignored drift test validates the current catalog hash file. |
| Skill routing, reference, content, install and provenance | PR #586 (routing, ownership, references), PR #590 (install state, removal), PR #593 (activated body) | https://github.com/agentmemoir/clio/pull/586 and https://github.com/agentmemoir/clio/pull/590 | PR #586 head 8f4413c; PR #590 head 56b8c6a; snapshot 7096d35 | 7096d35 for the routing half only | missing | Unverified | The routing, reference and content half is green at this head through `skill_drift` (30 passed). The install and provenance half has no same-head execution, and its earlier receipt is stale on two changed paths; the cause is in `## Incomplete receipt`. |
| Protocol and both transports | No merged feature change produced a scoped receipt; the same-head CI job is the producing receipt | https://github.com/agentmemoir/clio/actions/runs/42778397551/job/129702235248 | 7096d35 (tree 0de0469) | 7096d35; protocol revision `2026-07-28` | pass | Measured | `tests/conformance/main.rs` 8 passed (stdio, HTTP and revision pin), `mcp_read_transport_conformance` 5 passed, `mcp_principal_binding` 17 passed, and the lib run covers the protocol, HTTP, HTTP-candidate, stdio and stateless tests. |
| Rollback (fixed activation inverse) | PR #607 (activation withdrawal drill), merge commit is the build head | https://github.com/agentmemoir/clio/pull/607 and the committed transcript `crates/clio-mcp/tests/conformance/transcripts/2026-11-10-revert-drill.md` | drill commit `fcf4b2d8...` (tree `dd01ab70...`) in a throwaway clone | pre-cut source `1b9b3e51d7c18e76a9fc0dc1d96363ee00202f95` (tree `dd01ab70...`) | pass | Measured | The receipt is this head's own committed transcript: its blob is identical at `4867905` and `7096d35`. It proves restoration to the pre-cut revision 2025-11-25, not to the cut target 2026-07-28, and it reuses two manifest budget rows rather than re-measuring them. |
| Runtime maintenance and audit-item traceability | none at this head | none | unknown | unknown | missing | Unverified | No merged change produced a runtime-maintenance receipt or an audit-item mapping with build and source identity. The behaviour is only covered indirectly by the conformance lib run. The cause is in `## Incomplete receipt`. |
| Operator-visible delivered text (fragments) | PR #606 (fragment verification and four corrections) | https://github.com/agentmemoir/clio/actions/runs/42778397471/job/129702234783 and https://github.com/agentmemoir/clio/pull/606 | 7096d35 (tree 0de0469) | 7096d35; `changelog.d/` byte-identical to `457dc41` | pass | Measured | The same-head CI step `Changelog fragments check` (`make scripts-check-changelog`) is green, and `git diff --quiet 457dc41 7096d35 -- changelog.d` exits 0. The same-head step supersedes PR #606's earlier receipt. |

## Final smoke

This record owns one bounded final smoke with no earlier owner at this build head. Its command, output, and verdict will be recorded here.

## Limits

A run URL points at a workflow run, and GitHub may expire old run logs; the commit and tree ids are the durable identity. A category with only an earlier-head receipt stays incomplete here.

## Incomplete receipt

Two mandatory categories are incomplete at this build, so delivery is incomplete for them. Both are recorded as `missing` above and are not treated as passing.

Skill install and provenance. No same-head execution covers the paths that own install, uninstall and provenance. Those paths live in the `clio` crate, the conformance job runs only `-p clio-mcp`, and the scoped-coverage job selected zero packages. The earlier receipt is stale on two changed paths: `crates/clio/src/skill_install.rs` changed after that receipt, and `crates/clio/tests/fixtures/golden_cli_help.json` changed in the activation sweep.

Runtime maintenance and audit-item traceability. One named producing owner closed as no-change and the other has no run record, so no merged receipt exists. No job at this head asserts an audit-item mapping, and the runtime-maintenance behaviour is covered only indirectly by the conformance lib run.

A missing or red mandatory result ends in a terminal incomplete receipt. It names the exact missing receipt and its cause, and it waits on no interactive question.
