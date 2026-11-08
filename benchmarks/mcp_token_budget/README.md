# MCP token budget

## Purpose

This directory holds the one measurement path for MCP tool-surface budgets: the exact byte size and the exact cl100k_base token count of the schema pack that `clio mcp schema-export` prints for the shipped tool surface.

The point is that a budget number is never a guess. Token counts come from a pinned, checksum-verified encoding loaded from a file in this directory, never from a regex heuristic, a retry loop, or a network fetch, so two people measuring the same build get the same number and a reviewer can recompute it.

## The command

Run this from a checkout with a built binary:

```
scripts/mcp_token_budget.sh
```

That is the whole interface. It pipes `clio mcp schema-export` into `benchmarks/mcp_token_budget/measure.py` and prints the budget document on stdout; anything worth failing on goes to stderr with a nonzero exit code.

Two environment variables are honoured: `CLIO_BIN` names the binary to use explicitly (otherwise `target/release/clio` is tried first, then `target/debug/clio`), and `PYTHON` names the interpreter (default `python3`).

For a pack you saved earlier, run the measuring step directly: `python3 benchmarks/mcp_token_budget/measure.py --pack path/to/schema_pack.json`.

The gate runs beside it: `scripts/mcp_budget_check.sh` exports the pack from the built binary, measures it, and exits 0 when both arms stay inside the frozen baseline, 1 when either arm grew, and 2 when an input cannot be measured. This is the same command CI runs, so a local result and a CI result come from the same check. The always-loaded skill arm runs only when `CLIO_SKILL_ENTRY_POINT` is set: a local run needs you to set it, and in CI it runs only when the repository variable `CLIO_SKILL_ENTRY_POINT` is configured.

## Prerequisites

Build the CLI, because the command measures what the binary prints: `cargo build --locked --bin clio` (or `make build`).

Install the tokenizer package, pinned in `benchmarks/mcp_token_budget/requirements.txt`: `python3 -m pip install -r benchmarks/mcp_token_budget/requirements.txt`.

That is the entire setup: no model download, no tokenizer cache warm-up, and no network access at measurement time.

## Output contract

The command prints one JSON object with a fixed key order: `format` (`clio_mcp_token_budget`), `format_version` (`1`), `encoding` (`cl100k_base`), `tokenizer` (`implementation` plus `version`, taken from the loaded tokenizer), `asset` (`path` — the repo-relative asset path — plus its `sha256`), and `rows`.

Each row carries `row` (a stable id), `value` (an integer), `unit` (`bytes` or `tokens`), `label` (exactly one of `Measured`, `Vendor-measured`, `External`, `Inferred`, `Decision`, `Unverified`), and `input_sha256`, the digest of the exact bytes that were measured.

There are two rows today, always emitted in this order: `pack.bytes` — the length in bytes of the canonical pack, meaning sorted keys, no insignificant whitespace, UTF-8 — and `pack.tokens` — the cl100k_base token count of exactly those canonical bytes.

The rows are never a copy of the input file: the command reads the pack (by default from `clio mcp schema-export`, or from `--pack PATH`), parses it, and re-serializes the whole pack to the shipped compact JSON form — sorted keys, `(",", ":")` separators, UTF-8, no trailing newline — before measuring.

The bytes row measures that canonical form, which equals the CLI's stdout without its trailing newline: 105,771 bytes for the current pack.

A saved pack in any other formatting (for example the pretty-printed test fixture `crates/clio-mcp/tests/fixtures/schema_pack.json`) produces the same rows for the same semantic pack, because the measurement is defined by the canonical form, not by the input formatting.

Two runs of the command on one unchanged build are byte-identical: the document carries no timestamp, no date, no host name, no absolute path, and no ordering that depends on the environment.

Whatever the pack grows to, the output stays a flat list of rows that a reader can compare across builds without re-deriving anything.

## Offline and failure behavior

The command performs no network I/O and downloads nothing; the encoding is loaded from the committed asset, so it works in a sandbox with no route out and no warm cache.

The tokenizer is loaded before any output is produced, and the whole document is built in memory and written to stdout once, so a failure can never leave a partial or plausible-looking row behind.

Three failure modes all exit nonzero with a clear `mcp-token-budget: ...` message on stderr and zero rows on stdout: the `tiktoken` package is missing, the encoding asset is missing, and the encoding asset is corrupt, meaning its digest no longer matches the pinned value.

A missing or unbuildable CLI, a pack that is not valid JSON, a pack whose `format` is not `am_tool_schema_pack`, and a pack with no tools fail the same way, for the same reason: no measurement is better than a wrong one.

## Asset provenance

`benchmarks/mcp_token_budget/assets/cl100k_base.tiktoken` is the cl100k_base mergeable-rank table, committed to this repository so the measurement cannot drift with a package's bundled data or a remote fetch.

Its sha256 is `223921b76ee99bde995b7ff738513eef100fb51d18c93597a113bcffe865b2a7`.

Every run re-hashes the asset and refuses to measure unless the digest matches the pinned value, and the same digest is reported in the output's `asset.sha256` field so a reader can tie a number back to the exact table that produced it.

The tokenizer implementation and version are reported alongside it, in the `tokenizer` field, for the same reason.

## Scope

This directory now holds one budget check that fails the build on pack byte or token growth against the immutable frozen baseline: `benchmarks/mcp_token_budget/budget_check.py` measures a pack, resolves `client/synthetic-eager-client/turn/1/bytes` and the matching `/tokens` row from `baseline/frozen-v1.json` — a file whose own digest is pinned, so a moved baseline is refused instead of compared against — and exits 1 when either arm is larger than its row.

The preset targets are reported but not yet enforcing: `targets/frozen-v1.json` states one ceiling for each preset that carries a target and unit with the arithmetic that produced it, and `budget_targets.compare` reports each target beside its measured value and headroom. Every remaining ceiling rests on the shared full-surface baseline row, not on a per-preset row. The smallest preset carries no target: its size is a chosen value in the decision record, and nothing compares against it. `PRESET_TARGETS_ENFORCED` is `False`, so a miss appears in the report and never changes the exit code, and flipping that one constant is the whole enforcement change.

Two different quantities are both called headroom, and they point opposite ways. The record's `headroom` is `baseline_value - target`, the room a target leaves under its frozen baseline, and `budget_targets` refuses a target whose headroom does not equal that subtraction. The report's per-target `headroom` is `target - measured`, the room the measured pack leaves under the target, so a negative value means the measured pack is over its target while a positive one means it still fits.

The always-loaded skill arm runs when a skill path is supplied: `--skill PATH` measures that file's raw bytes, its UTF-8 text, and its cl100k_base token count, and checks it against the frozen `retired_skill_entry_point/bytes` (6393) and `retired_skill_entry_point/tokens` (1518) rows. That arm takes no allowance, because an allowance names a pack digest, so a skill file that grows past either row fails the same run with exit 1 and a message naming that exact row.

In CI the path comes from the `CLIO_SKILL_ENTRY_POINT` repository variable, which the budget job passes to `budget_check.py` only when it is set, because the entry point lives outside this repository and no public checkout can hard-code it. When the variable is unset the report's `skill` object says `not-measured` and still prints both frozen row values under `baseline_rows`, so an unmeasured arm is never read as a zero cost. An explicitly empty `--skill` path is refused with exit 2, because an empty argument is a wiring mistake rather than a decision to skip the arm.

The shipped activated skill body has its own frozen row, `activated_skill_body/tokens`, fixed at 594 tokens and measured from `skills/clio-mcp/SKILL.md` (2426 bytes, sha256 `09b61130277acd22823f2ad5d3fd3c5f329df1e34f69bf5c07084b9b0a407866`) with the pinned offline tokenizer: cl100k_base, tiktoken 0.14.0, and the pinned rank-table digest. The row carries the role `shipped-activated-skill-body`, the repo-relative file it measures, the label `Measured`, `tokenizer_ref` set to `pinned`, and a factor of `1.0`.

That row is separate from the `retired_skill_entry_point` rows and separate from the per-preset listing reduction targets: it is the cost of the activated body, not proof that a client always loads it.

The check reads the frozen row on every run and derives the ceiling from the row alone, as value times factor, with the factor required to be exactly `1.0`. A row that is missing, duplicated, non-positive, in another unit, or that states any other factor is refused by name instead of being replaced by a default. Nothing in the check writes, so a candidate measurement can never move a later ceiling.

The shipped body is measured by default; `--activated-skill PATH` measures another file in its place, and an explicitly empty path is a usage error that exits 2. The arm passes when the candidate token count is at or below the ceiling. Token growth fails the run and names `activated_skill_body/tokens` with the excess, while a byte or digest change that leaves the count unchanged still passes, because the bound is token growth and not content identity. The report carries this as an `activated` object, a top-level key beside `skill`, holding the frozen digest, the candidate digest, whether the two match, and the numbers behind the verdict.

Growth is accepted only by an exact-digest allowance record with both ceilings: `allowances/frozen-v1.json` holds the records, one record names one candidate pack digest together with `max_bytes` and `max_tokens`, and a growth with no record naming its digest — or with only one ceiling, or with both ceilings exceeded — fails and names the measure, the numbers, and the digest. The two arms stay independent under a record: `budget_allowances.within` reports problems keyed by measure, so a byte ceiling that is breached fails the bytes arm while a token count that still fits its own ceiling keeps the tokens arm passing, and each failure message names only its own measure.

An allowance record carries a stable `decision_id`, the `date` the decision was made, a `reason`, the measured `base_commit`, the exact `candidate_sha256` pack digest it covers, and its independent `max_bytes` and `max_tokens` ceilings. The commit the allowance lands in is deliberately not a field: git history records it, so the record cannot go stale by naming a commit that does not exist yet. The record grants no headroom: a grown pack has a different canonical digest, so growth needs its own record.

Chosen values land in the decisions record: [`decisions/frozen-v1.json`](decisions/frozen-v1.json) holds the ordered records that `budget_decisions.py` validates, and each one names the preset, the bytes and tokens chosen for it, the `reason` for choosing that size, and the frozen presets row the value is derived from, so a chosen number is traceable to a measurement instead of being stated on trust. The exception is the target factor decision: [`targets/frozen-v1.json`](targets/frozen-v1.json) keeps its `factor_decision` labelled `Decision` because every target must match it, so that one is gate arithmetic and stays in the target record. A Decision is never a measurement and never a gate input, so no chosen value can loosen or tighten a budget check. The record names the measured `base_commit` it was derived on, while git history supplies the commit it lands in, because a record that named its own landing commit would go stale the moment it moved.

Keeping the measurement and the judgement apart is deliberate, and it still holds inside the one check: `measure.py` prints the rows for a human or a review and carries no threshold, while the judgement lives in `budget_check.py` against the pinned digest, so a measurement that can be argued with stays trustworthy and a threshold can move later without re-deriving how the number was produced.
