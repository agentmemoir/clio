# clio-bench scorecard

- Suite(s): longmemeval
- Partition: canary (seed 100480, max probes 6)
- Adapter: clio
- Judge: qwen2.5-1.5b-instruct (tier1)
- Generator: qwen2.5-1.5b-instruct
- Tokenizer: cl100k_base(clio-bench-fallback)
- Generated: 2026-09-24T16:36:40.895381+00:00

## longmemeval

Scored probes: 6 (failed 0); overall accuracy 0.0000; token F1 0.1526
Ingestion: 6 instances, 96 turns (1 rejected by admission)

| Category | Count | Accuracy | Token F1 | p95 retrieval ms |
|---|---|---|---|---|
| knowledge-update | 1 | 0.0000 | 0.2000 | 659.9 |
| single-hop | 4 | 0.0000 | 0.0789 | 472.8 |
| temporal-reasoning | 1 | 0.0000 | 0.4000 | 965.5 |

