# am-bench scorecard

- Suite(s): locomo
- Partition: canary (seed 100480, max probes 20)
- Adapter: clio
- Judge: stub-exact-match (stub)
- Generator: stub-context-echo
- Tokenizer: cl100k_base(am-bench-fallback)
- Generated: 2026-10-07T00:34:58.392055+00:00

## Configuration

### clio 0.1.2

- `retention_profile`: `{"accepted_but_inert": {"duplicate_tolerance_read": "lenient", "honored_by": "recall_scope_and_dedup", "note": "read-si…` (read through `clio retention get --bank am-bench-fingerprint (store named by CLIO_RETENTION_PROFILES)`)
- `ranking_environment`: `{"admission": {"w1": 0.2, "w2": 0.2, "w3": 0.2, "w4": 0.2, "w5": 0.2}, "coactivation": {"eta": 0.15, "half_life_days": …` (read through `clio ranking get --output json`)
- `trigger_state`: not readable; tried clio maintenance status --output json (index pipeline fields only; no trigger state field exists yet)
- `engine_profile`: not readable; tried clio config get engine_profile (unknown config path `engine_profile`)

## locomo

Scored probes: 20 (failed 0) [tier stub, 20 probes; not quotable]; overall accuracy 0.0000; token F1 0.0243
Ingestion: 2 instances, 928 turns (0 rejected by admission)

| Category [tier stub, 20 probes; not quotable] | Count | Accuracy | Token F1 | p95 retrieval ms |
|---|---|---|---|---|
| adversarial-abstention | 4 | 0.0000 | 0.0000 | 168.5 |
| multi-hop | 4 | 0.0000 | 0.0690 | 180.5 |
| open-domain | 4 | 0.0000 | 0.0000 | 177.2 |
| single-hop | 4 | 0.0000 | 0.0363 | 166.5 |
| temporal-reasoning | 4 | 0.0000 | 0.0161 | 166.0 |

