# clio-bench

A thin, reproducible retrieval-quality harness for Clio. It scores two long-term-memory suites — a LongMemEval-style suite and a LoCoMo-style suite — with one command against pinned snapshots, using a calibrated LLM-as-a-judge.

The harness is additive. It drives the public `clio` binary through a system adapter and never changes retrieval, extraction, or admission behavior.

## Layout

```
benchmarks/
├── run.py                     # one-command entrypoint (one system)
├── baseline.py                # one-command entrypoint (both real systems)
├── README.md
├── clio_bench/
│   ├── adapter.py             # unified system-adapter interface
│   ├── packing.py             # budget-aware packing of retrieval hits
│   ├── fingerprint.py         # shared configuration-fingerprint shape
│   ├── types.py               # shared data model
│   ├── partition.py           # 60/20/20 and 5-fold session-isolated splits
│   ├── tokenizer.py           # pinned cl100k_base reference tokenizer
│   ├── scoring.py             # token F1 and score aggregation
│   ├── fetch.py               # pinned download + hash verification
│   ├── llm.py                 # OpenAI-compatible chat client (stdlib)
│   ├── answerer.py            # context-only answer generator
│   ├── doubles.py             # deterministic stub judge/generator
│   ├── runner.py              # suite loading and the run loop
│   ├── report.py              # report assembly and Markdown scorecard
│   ├── cli.py                 # command line: flags, registry wiring, output
│   ├── baseline.py            # two-system baseline and blocked-gate record
│   ├── stubs.py               # BEAM / MemoryAgentBench / AMA-Bench stubs
│   ├── pins.json              # dataset pins and judge settings
│   ├── datasets/              # locomo.py, longmemeval.py
│   ├── adapters/              # clio.py, mnemosyne.py, memory.py
│   └── judges/                # llm_judge.py
└── tests/                     # standard-library unittest suite
```

## Unified adapter interface

Every benchmarked system implements `MemoryBenchmarkAdapter`:

| Method | Purpose |
|---|---|
| `reset(session_id)` | Prepare a fresh, isolated memory bank for a test case |
| `ingest(session_id, turn) -> IngestResult` | Ingest one dialogue turn; return latency, tokens, success |
| `retrieve(session_id, query, token_budget) -> RetrievalResult` | Return a context string within a hard token budget |
| `answer(session_id, query, generator_llm) -> str` | Optional end-to-end answering |
| `healthcheck() -> bool` | Whether the system is ready to be driven |
| `fingerprint() -> Fingerprint` | The behaviour-changing settings, captured before any probe runs |

`adapters/clio.py` drives the `clio` binary (`remember`, `recall`, `health`); `adapters/memory.py` is a deterministic in-memory adapter used for offline runs and tests; `adapters/mnemosyne.py` drives the `mnemosyne` CLI.

## Configuration fingerprint

Every report carries, per system, the settings that change that system's behaviour. Each adapter implements `fingerprint()` and returns one structured block, `{system, version, settings}`, captured before any probe runs and stamped into the report under `systems.<name>`:

- Each readable setting records `{"status": "read", "value": ..., "surface": ...}`.
- A setting with no read surface records `{"status": "unreadable", "tried": [...]}` and is never omitted.
- Every system has a minimum: Clio must read its retention profile and ranking environment, and Mnemosyne must read the ranking levers its `config get` surface exposes. A run whose fingerprint leaves a required setting unreadable exits 2 and writes no report.

The fingerprint records settings only. Keys and tokens are named by their environment variable and never echoed; the blocked-gate record says whether a key variable is set, never its value.

## Tier discipline

Only tier 2 may be quoted as a result. Every quoted number names its tier and its probe count on the same line, in both the JSON aggregate blocks (`tier`, `quotable`, and `scored_probes`) and the Markdown scorecard (`[tier <tier>, <n> probes]`, with `; not quotable` when the tier is not tier 2). Tier-1 and stub numbers are for the inner loop and are never quoted as results.

## Pinned snapshots

Datasets are downloaded at run time and never committed. Each download is verified against the pin in `clio_bench/pins.json`; a mismatch aborts the run before any score is written.

| Suite | Pin | License |
|---|---|---|
| LoCoMo-style | commit `3eb6f2c…`, blob `d95b8724…` | CC BY-NC 4.0 (no redistribution) |
| LongMemEval-style | sha256 `821a2034…` (oracle split) | MIT |

## Partitioning

`partition.py` splits whole instances (a conversation for LoCoMo, a haystack item for LongMemEval) into a deterministic 60% train / 20% canary / 20% blind split, and supports 5-fold rotation for smaller datasets. No turn or probe from one instance ever crosses a partition boundary.

## Judge

`judges/llm_judge.py` implements the calibrated two-tier judge. It emits a structured JSON verdict (`accuracy_score`, `temporal_correctness`, `hallucination_penalty`, `explanation`) at `temperature = 0.0`. To cancel positional bias, each probe is judged twice with the candidate and reference swapped and the two accuracy scores combined with a geometric mean. If a pass is not valid JSON, the judge raises and the run aborts — no score is fabricated.

Judge settings live in `pins.json`:

- Tier 1 (local, inner loop): `qwen2.5-1.5b-instruct` at `http://127.0.0.1:8899`.
- Tier 2 (official): `gpt-4o-2024-08-06`. Credentials come from `CLIO_BENCH_API_KEY` or `OPENAI_API_KEY` and are never logged.

## Tokenizer

`tokenizer.py` pins the `cl100k_base` encoding. When the `tiktoken` package is installed the harness uses it; otherwise it falls back to a deterministic, documented estimator and records `cl100k_base(clio-bench-fallback)` in every report so token numbers are never silently incomparable.

## Running

From the repository root:

```bash
# Offline smoke run (deterministic in-memory adapter, stub judge)
python3 benchmarks/run.py --suite both --adapter memory --judge stub \
    --generator stub --max-probes 8

# Real run against Clio with the local judge
python3 benchmarks/run.py --suite both --partition canary --max-probes 12 \
    --adapter clio --judge local --generator local \
    --out benchmarks/reports/run-clio.json \
    --report-md benchmarks/reports/run-clio.md
```

Inputs: a pinned suite snapshot, a partition, an adapter, a judge, and a generator. Outputs: a JSON report (per-probe records, per-category and overall accuracy, token F1, latency percentiles) and an optional Markdown scorecard. Point `--data-dir` at an absolute path: clio refuses a relative one and its writes fail.

## The two-system baseline

One command runs both real systems on the same pinned inputs and writes one report whose every number names its tier and probe count:

```bash
python3 benchmarks/baseline.py --suite both --partition canary \
    --judge tier2 --generator tier2 \
    --clio-binary <clio> --mnemosyne-binary <mnemosyne> \
    --out benchmarks/reports/baseline.json \
    --report-md benchmarks/reports/baseline.md
```

The baseline defaults to the whole canary partition at tier 2. A missing system is a stop condition: no baseline is written when either leg fails.

If no frontier key (`CLIO_BENCH_API_KEY` or `OPENAI_API_KEY`) is set, the command writes a blocked-gate record instead and exits 2: no probe runs, no tier-1 number is substituted, and both systems' fingerprints are attached so the configuration is still recorded. The record names the gate (`quotable-baseline`), the systems, and whether each key variable is set.

List the documented stubs:

```bash
python3 benchmarks/run.py --list-stubs
```

## Tests

```bash
python3 -m unittest discover -s benchmarks/tests -t .
```

## Stubs (not implemented)

BEAM, MemoryAgentBench, and AMA-Bench are documented stubs only. Requesting one of them fails clearly and produces no score. A later phase extends the same adapter interface to them.

## Known limitations

- Only two suites are implemented; the other three are stubs.
- Recorded baseline scores depend on the judge model; judge drift changes comparability.
- Continuous CI benchmarking is not wired.
- When `tiktoken` is absent, token metrics use the documented fallback estimator rather than the exact pinned encoding.
- Clio's trigger state and engine profile have no read surface in this build; both are recorded as explicitly unreadable with the commands tried.
- A tier-2 baseline needs a frontier key. With none, the report is the blocked record and no score exists to quote.

## Security

- No dataset is committed; snapshots are fetched and hash-verified at run time.
- Judge credentials come from the environment and are never written to logs or reports. Reports contain scores and public benchmark text only.
