# Two-profile am-bench runs: `long-lived-companion` vs `coding-agent`

These are smoke runs. Both ran with `--judge stub --generator stub` because no judge or generator API key is set on this machine. Stub scores test wiring only. The numbers in the two scorecards are smoke signals and must never be presented as results.

## What ran

Both runs are identical in every harness setting except the applied profile. Same binary `target/debug/clio` (clio 0.1.2), same suite `locomo`, same partition `canary`, same seed 100480, same caps `--max-instances 2 --max-probes 20`, same `--judge stub --generator stub`, same adapter `clio`, same token budget 4096, same default data directory `benchmarks/.cache/clio-data`, and same profile store `benchmarks/.cache/profiles-store.json` named by `CLIO_RETENTION_PROFILES`.

Profile application was the only difference:

- Companion run: `target/debug/clio retention set --profiles benchmarks/.cache/profiles-store.json --scope deployment --profile-file benchmarks/profiles/long-lived-companion.json`
- Coding run: `target/debug/clio retention set --profiles benchmarks/.cache/profiles-store.json --scope deployment --profile-file benchmarks/profiles/coding-agent.json`

The adapter reads the store through `clio retention get --bank am-bench-fingerprint`, so the profile reaches every `clio` subprocess the adapter spawns.

## Probe and instance counts, and why they are capped

Both reports used 20 probes over 2 instances (`conv-26` then `conv-49`). The stub tier cannot justify a constant, so a constant is not claimed. The runs are capped for time and both runs use the same caps.

## Profile reached the system (fingerprint)

Both reports confirm the profile arrived. The two values differ in the fields the profiles are supposed to change.

Companion: `verbosity` `balanced`, `duplicate_tolerance_write` `lenient`, `accepted_but_inert.duplicate_tolerance_read` `lenient`, `accepted_but_inert.recall_scope_default` `full`.

Coding: `verbosity` `selective`, `duplicate_tolerance_write` `strict`, `accepted_but_inert.duplicate_tolerance_read` `strict`, `accepted_but_inert.recall_scope_default` `consolidated_only`.

The full `systems.clio.settings` blocks are copied verbatim below.

### Companion `systems.clio.settings`

```json
{
  "retention_profile": {
    "status": "read",
    "value": {
      "bank": "am-bench-fingerprint",
      "source": "deployment",
      "verbosity": "balanced",
      "theta_offsets": {},
      "duplicate_tolerance_write": "lenient",
      "version": 3,
      "accepted_but_inert": {
        "duplicate_tolerance_read": "lenient",
        "honored_by": "recall_scope_and_dedup",
        "note": "read-side scope and dedup are active; these values apply to retrieve/compose unless the call overrides them",
        "recall_scope_default": "full",
        "status": "active"
      }
    },
    "surface": "clio retention get --bank am-bench-fingerprint (store named by CLIO_RETENTION_PROFILES)"
  },
  "ranking_environment": {
    "status": "read",
    "value": {
      "admission": {"w1": 0.2, "w2": 0.2, "w3": 0.2, "w4": 0.2, "w5": 0.2},
      "coactivation": {"eta": 0.15, "half_life_days": 30.0, "hub_degree_limit": 16, "max_hops": 2, "w_min": 0.05},
      "hygiene": {"dup": 0.15, "pattern": 0.25, "secret": 0.35, "staleness": 0.1, "utility": 0.15},
      "hygiene_stale_half_life_days": 90.0,
      "intent_gate": {"skip_confidence": null},
      "maintenance_triggers": {"cooldown_seconds": 300, "dirty_ancestor_threshold": 1, "eligible_hub_threshold": 1, "hub_budget_per_wave": 16, "max_waves_per_hour": 12, "stale_unit_threshold": 1},
      "ok": true,
      "recency_tau_days": 30.0,
      "retention": {"half_life_days": 30.0},
      "retrieval": {"dense": 0.3333333333333333, "importance": 0.3333333333333333, "lexical": 0.3333333333333333},
      "theta_admit": 0.55,
      "theta_overrides": {},
      "trust": {"agent_inferred": 0.85, "third_party": 0.95, "user_stated": 1.0},
      "type_prior_overrides": {}
    },
    "surface": "clio ranking get --output json"
  },
  "trigger_state": {"status": "unreadable", "tried": ["clio maintenance status --output json (index pipeline fields only; no trigger state field exists yet)"]},
  "engine_profile": {"status": "unreadable", "tried": ["clio config get engine_profile (unknown config path `engine_profile`)"]}
}
```

### Coding `systems.clio.settings`

```json
{
  "retention_profile": {
    "status": "read",
    "value": {
      "bank": "am-bench-fingerprint",
      "source": "deployment",
      "verbosity": "selective",
      "theta_offsets": {},
      "duplicate_tolerance_write": "strict",
      "version": 3,
      "accepted_but_inert": {
        "duplicate_tolerance_read": "strict",
        "honored_by": "recall_scope_and_dedup",
        "note": "read-side scope and dedup are active; these values apply to retrieve/compose unless the call overrides them",
        "recall_scope_default": "consolidated_only",
        "status": "active"
      }
    },
    "surface": "clio retention get --bank am-bench-fingerprint (store named by CLIO_RETENTION_PROFILES)"
  },
  "ranking_environment": {
    "status": "read",
    "value": {
      "admission": {"w1": 0.2, "w2": 0.2, "w3": 0.2, "w4": 0.2, "w5": 0.2},
      "coactivation": {"eta": 0.15, "half_life_days": 30.0, "hub_degree_limit": 16, "max_hops": 2, "w_min": 0.05},
      "hygiene": {"dup": 0.15, "pattern": 0.25, "secret": 0.35, "staleness": 0.1, "utility": 0.15},
      "hygiene_stale_half_life_days": 90.0,
      "intent_gate": {"skip_confidence": null},
      "maintenance_triggers": {"cooldown_seconds": 300, "dirty_ancestor_threshold": 1, "eligible_hub_threshold": 1, "hub_budget_per_wave": 16, "max_waves_per_hour": 12, "stale_unit_threshold": 1},
      "ok": true,
      "recency_tau_days": 30.0,
      "retention": {"half_life_days": 30.0},
      "retrieval": {"dense": 0.3333333333333333, "importance": 0.3333333333333333, "lexical": 0.3333333333333333},
      "theta_admit": 0.55,
      "theta_overrides": {},
      "trust": {"agent_inferred": 0.85, "third_party": 0.95, "user_stated": 1.0},
      "type_prior_overrides": {}
    },
    "surface": "clio ranking get --output json"
  },
  "trigger_state": {"status": "unreadable", "tried": ["clio maintenance status --output json (index pipeline fields only; no trigger state field exists yet)"]},
  "engine_profile": {"status": "unreadable", "tried": ["clio config get engine_profile (unknown config path `engine_profile`)"]}
}
```

## Inventory after each run

The adapter deletes the whole `clio.db` on each `reset`, so only the last instance's bank survives a run. Each run processed `conv-26` first and `conv-49` second, so `am-bench-conv-49` is the surviving bank.

The bare inventory command in the procedure, `target/debug/clio stats --bank am-bench-<id> --output json`, read the default data directory, not the run directory, and returned `items_total` 0 for both banks. Clio finds its database through `CLIO_DATA_DIR`, and the adapter sets that to `benchmarks/.cache/clio-data`. So the inventory was repeated with `CLIO_DATA_DIR=benchmarks/.cache/clio-data`, which read the run database. Both command forms and their results are recorded in `06-runs.json`.

Companion run, `am-bench-conv-26`: 0 items via both forms. `am-bench-conv-49`: 0 via the bare form, 509 items via the run directory (`items_active` 509, `items_discarded` 0, all `output_constraint` / `fact` / `semantic`).

Coding run, `am-bench-conv-26`: 0 items via both forms. `am-bench-conv-49`: 0 via the bare form, 508 items via the run directory (`items_active` 508, `items_discarded` 0, all `output_constraint` / `fact` / `semantic`).

These counts are store state, not scores. Do not read them as quality.

## Machine state

Before both runs (`2026-10-07T00:33:27Z`): `nproc` 4, load average 0.48, 0.51, 0.54, up 13:41.

After both runs (`2026-10-07T00:36:51Z`): `nproc` 4, load average 1.38, 1.00, 0.74, up 13:44.

## Caveats

- Both scorecards come from `--judge stub --generator stub`. Their accuracy and token F1 values are smoke signals, not results.
- Half-life is not part of either profile in this build. Both runs used the system default half-life of 30 days, as the profiles README states.
- `turns_rejected` differed (0 for the companion, 1 for the coding agent). That is a smoke observation from the stub paths; it is not evidence about profile quality.
