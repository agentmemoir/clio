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

## Prerequisites

Build the CLI, because the command measures what the binary prints: `cargo build --locked --bin clio` (or `make build`).

Install the tokenizer package, pinned in `benchmarks/mcp_token_budget/requirements.txt`: `python3 -m pip install -r benchmarks/mcp_token_budget/requirements.txt`.

That is the entire setup: no model download, no tokenizer cache warm-up, and no network access at measurement time.

## Output contract

The command prints one JSON object with a fixed key order: `format` (`clio_mcp_token_budget`), `format_version` (`1`), `encoding` (`cl100k_base`), `tokenizer` (`implementation` plus `version`, taken from the loaded tokenizer), `asset` (`path` — the repo-relative asset path — plus its `sha256`), and `rows`.

Each row carries `row` (a stable id), `value` (an integer), `unit` (`bytes` or `tokens`), `label` (exactly one of `Measured`, `Vendor-measured`, `External`, `Inferred`, `Decision`, `Unverified`), and `input_sha256`, the digest of the exact bytes that were measured.

There are two rows today, always emitted in this order: `pack.bytes` — the length in bytes of the canonical pack, meaning sorted keys, no insignificant whitespace, UTF-8 — and `pack.tokens` — the cl100k_base token count of exactly those canonical bytes.

Canonicalization matters: pretty-printing the same pack yields more bytes, so both rows are computed over the canonical form, which is byte-for-byte what `clio mcp schema-export` writes minus its trailing newline.

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

This directory measures and reports; it holds no gate, no threshold, no budget limit, and no pass/fail verdict.

Nothing here fails a build because a number got bigger. The rows exist so a human, a review, or a separate policy can see what the tool surface actually costs and decide what to do about it.

Keeping the measurement and the judgement apart is deliberate: a measurement that can be argued with stays trustworthy, and a threshold can move later without re-deriving how the number was produced.
