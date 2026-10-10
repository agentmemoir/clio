# clio-bench scorecard

- Suite(s): locomo
- Partition: canary (seed 100480, max probes 5)
- Adapter: clio
- Judge: qwen2.5-1.5b-instruct (tier1)
- Generator: qwen2.5-1.5b-instruct
- Tokenizer: cl100k_base(clio-bench-fallback)
- Generated: 2026-09-24T16:31:42.505395+00:00

## locomo

Scored probes: 5 (failed 0); overall accuracy 0.2265; token F1 0.2127
Ingestion: 1 instances, 509 turns (0 rejected by admission)

| Category | Count | Accuracy | Token F1 | p95 retrieval ms |
|---|---|---|---|---|
| adversarial-abstention | 1 | 0.0000 | 0.0000 | 1532.6 |
| multi-hop | 1 | 0.6325 | 0.3137 | 1502.3 |
| open-domain | 1 | 0.0000 | 0.1053 | 1448.0 |
| single-hop | 1 | 0.5000 | 0.4444 | 1544.5 |
| temporal-reasoning | 1 | 0.0000 | 0.2000 | 1533.5 |

