# am-bench scorecard

- Suite(s): locomo
- Partition: canary (seed 100480, max probes 20)
- Adapter: clio
- Judge: stub-exact-match (stub)
- Generator: stub-context-echo
- Tokenizer: cl100k_base(am-bench-fallback)
- Generated: 2026-10-07T00:36:44.619146+00:00

## Configuration

### clio 0.1.2

- `retention_profile`: `{"accepted_but_inert": {"duplicate_tolerance_read": "strict", "honored_by": "recall_scope_and_dedup", "note": "read-sid…` (read through `clio retention get --bank am-bench-fingerprint (store named by CLIO_RETENTION_PROFILES)`)
- `ranking_environment`: `{"admission": {"w1": 0.2, "w2": 0.2, "w3": 0.2, "w4": 0.2, "w5": 0.2}, "coactivation": {"eta": 0.15, "half_life_days": …` (read through `clio ranking get --output json`)
- `trigger_state`: not readable; tried clio maintenance status --output json (index pipeline fields only; no trigger state field exists yet)
- `engine_profile`: not readable; tried clio config get engine_profile (unknown config path `engine_profile`)

## locomo

Scored probes: 20 (failed 0) [tier stub, 20 probes; not quotable]; overall accuracy 0.2000; token F1 0.0053
Ingestion: 2 instances, 928 turns (1 rejected by admission)

| Category [tier stub, 20 probes; not quotable] | Count | Accuracy | Token F1 | p95 retrieval ms |
|---|---|---|---|---|
| adversarial-abstention | 4 | 1.0000 | 0.0000 | 149.4 |
| multi-hop | 4 | 0.0000 | 0.0263 | 166.3 |
| open-domain | 4 | 0.0000 | 0.0000 | 166.7 |
| single-hop | 4 | 0.0000 | 0.0000 | 166.7 |
| temporal-reasoning | 4 | 0.0000 | 0.0000 | 155.3 |

