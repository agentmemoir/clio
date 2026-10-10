# clio-bench blocked gate

- Gate: quotable-baseline
- Reason: frontier judge key absent
- Systems: clio, mnemosyne
- Judge requested: tier2
- Generator requested: tier2
- Key variables required: CLIO_BENCH_API_KEY, OPENAI_API_KEY
- Variables set: CLIO_BENCH_API_KEY=False, OPENAI_API_KEY=False
- Detail: the two-system baseline needs a tier-2 judge and no frontier key is set; no probe was run, no score was produced, and tier 1 was not substituted

No probe was run, no score was produced, and tier 1 was not substituted.

## Configuration

### clio 0.1.2

- `retention_profile`: `{"accepted_but_inert": {"duplicate_tolerance_read": "balanced", "honored_by": "recall_scope_and_dedup", "note": "read-s…` (read through `clio retention get --bank clio-bench-fingerprint (no store configured; builtin profile at the pinned benchmark data directory)`)
- `ranking_environment`: `{"admission": {"w1": 0.2, "w2": 0.2, "w3": 0.2, "w4": 0.2, "w5": 0.2}, "coactivation": {"eta": 0.15, "half_life_days": …` (read through `clio ranking get --output json`)
- `trigger_state`: not readable; tried clio maintenance status --output json (index pipeline fields only; no trigger state field exists yet)
- `engine_profile`: not readable; tried clio config get engine_profile (unknown config path `engine_profile`)

### mnemosyne 3.15.1

- `vec_weight`: `0.5` (read through `mnemosyne config get vec_weight`)
- `fts_weight`: `0.3` (read through `mnemosyne config get fts_weight`)
- `importance_weight`: `0.2` (read through `mnemosyne config get importance_weight`)
- `recency_halflife`: `168` (read through `mnemosyne config get recency_halflife`)
- `tier2_days`: `30` (read through `mnemosyne config get tier2_days`)
- `tier2_weight`: `0.5` (read through `mnemosyne config get tier2_weight`)
- `tier3_days`: `180` (read through `mnemosyne config get tier3_days`)
- `tier3_weight`: `0.25` (read through `mnemosyne config get tier3_weight`)
- `wm_max_items`: `10000` (read through `mnemosyne config get wm_max_items`)
- `wm_ttl_hours`: `168` (read through `mnemosyne config get wm_ttl_hours`)
- `cross_session`: `false` (read through `mnemosyne config get cross_session`)
- `embedding_model`: `"BAAI/bge-small-en-v1.5"` (read through `mnemosyne config get embedding_model`)
- `embedding_dim`: `384` (read through `mnemosyne config get embedding_dim`)
- `vec_type`: `"int8"` (read through `mnemosyne config get vec_type`)

