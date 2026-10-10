# Two profiles compared: long-lived companion vs coding agent

This report compares two smoke runs of the am-bench harness, one run per retention deployment profile. It records what the harness saw. It is not a quality result. The am-bench tier rules say only tier 2 may be quoted, and both runs are tier `stub`.

## Status

Two deployment profiles landed: `long-lived-companion` and `coding-agent`. Two harness runs were taken, one per profile, and they held every harness setting equal except the applied profile. Both runs used the stub judge and generator because no frontier API key exists on this machine, so there is no quotable baseline and every score below is a labelled smoke signal. No constant was changed. Read the comparison as limited to fingerprint-visible fields: the fingerprint diff cannot witness two of the six profile differences (`retention_floor` and `provenance_floor_default`), and the run cannot show trigger state, so maintenance is unobserved.

## What ran

Both runs used the same suite (`locomo`), the same partition (`canary`), the same judge and generator (`stub`), the same caps (`--max-instances 2 --max-probes 20`), the same seed (`100480`), the same binary (`target/debug/clio`, clio 0.1.2), and the same run data directory (`benchmarks/.cache/clio-data`).

The profile store path was `benchmarks/.cache/profiles-store.json`, named by `CLIO_RETENTION_PROFILES`, and the adapter read the profile per subprocess through `clio retention get --bank am-bench-fingerprint`.

The companion run:

```bash
target/debug/clio retention set --profiles benchmarks/.cache/profiles-store.json --scope deployment --profile-file benchmarks/profiles/long-lived-companion.json
env CLIO_RETENTION_PROFILES=benchmarks/.cache/profiles-store.json python3 benchmarks/run.py --adapter clio --suite locomo --partition canary --judge stub --generator stub --max-instances 2 --max-probes 20 --binary target/debug/clio --out benchmarks/reports/profile-long-lived-companion.json --report-md benchmarks/reports/profile-long-lived-companion.md
```

The coding run:

```bash
target/debug/clio retention set --profiles benchmarks/.cache/profiles-store.json --scope deployment --profile-file benchmarks/profiles/coding-agent.json
env CLIO_RETENTION_PROFILES=benchmarks/.cache/profiles-store.json python3 benchmarks/run.py --adapter clio --suite locomo --partition canary --judge stub --generator stub --max-instances 2 --max-probes 20 --binary target/debug/clio --out benchmarks/reports/profile-coding-agent.json --report-md benchmarks/reports/profile-coding-agent.md
```

The two output paths are `benchmarks/reports/profile-long-lived-companion.json` and `benchmarks/reports/profile-coding-agent.json`.

The machine state before both runs was recorded at `2026-10-07T00:33:27Z`: `nproc` 4 and load average 0.48, 0.51, 0.54. After both runs, at `2026-10-07T00:36:51Z`: `nproc` 4 and load average 1.38, 1.00, 0.74.

## Fingerprint diff

`benchmarks/profile_diff.py` computed the diff from the two report JSONs with `changed_settings(settings_of(companion), settings_of(coding))`. The only setting that changed is `retention_profile`.

The changed inner fields, with the companion value first and the coding value second:

- `verbosity`: `balanced` vs `selective`.
- `duplicate_tolerance_write`: `lenient` vs `strict`.
- `accepted_but_inert.duplicate_tolerance_read`: `lenient` vs `strict`.
- `accepted_but_inert.recall_scope_default`: `full` vs `consolidated_only`.

The check `outside_profile(changed_settings(...))` is empty, so nothing outside the named profile setting changed between the two runs.

This check has a blind spot, and it is stated here plainly. The fingerprint reads the retention profile through `clio retention get`, which omits `retention_floor`, `provenance_floor_default`, `retention_curve`, `pack_fidelity_default`, and `maintenance_trigger`. The two profile files set `retention_floor` and `provenance_floor_default` differently, and the system resolves the profile (the `retention set` output echoed both values), but `clio retention get` does not echo them, so the fingerprint cannot show that difference, and this report does not claim the fingerprint proves it. A second gap: the adapter hardcodes `trigger_state` as unreadable even though `maintenance status` now returns a trigger block, so the run cannot say whether maintenance ran.

## Per-category comparison

Every number below sits on the same line as its tier and probe count. The label `[tier stub, 20 probes; not quotable]` is the suite aggregate, and `category count 4` is the number of probes in that category. No stub number may be presented as a result.

Companion profile (`long-lived-companion`):

- adversarial-abstention: accuracy 0.0000, token F1 0.0000, category count 4 [tier stub, 20 probes; not quotable]
- multi-hop: accuracy 0.0000, token F1 0.0690, category count 4 [tier stub, 20 probes; not quotable]
- open-domain: accuracy 0.0000, token F1 0.0000, category count 4 [tier stub, 20 probes; not quotable]
- single-hop: accuracy 0.0000, token F1 0.0363, category count 4 [tier stub, 20 probes; not quotable]
- temporal-reasoning: accuracy 0.0000, token F1 0.0161, category count 4 [tier stub, 20 probes; not quotable]

Coding profile (`coding-agent`):

- adversarial-abstention: accuracy 1.0000, token F1 0.0000, category count 4 [tier stub, 20 probes; not quotable]
- multi-hop: accuracy 0.0000, token F1 0.0263, category count 4 [tier stub, 20 probes; not quotable]
- open-domain: accuracy 0.0000, token F1 0.0000, category count 4 [tier stub, 20 probes; not quotable]
- single-hop: accuracy 0.0000, token F1 0.0000, category count 4 [tier stub, 20 probes; not quotable]
- temporal-reasoning: accuracy 0.0000, token F1 0.0000, category count 4 [tier stub, 20 probes; not quotable]

The suite overall was accuracy 0.0000, token F1 0.0243 for the companion and accuracy 0.2000, token F1 0.0053 for the coding profile [tier stub, 20 probes; not quotable].

The coding run returned an empty context on every probe: all 20 probes in `profile-coding-agent.json` show `retrieved_context` empty and `tokens_retrieved` `0`. The companion run retrieved context on all 20 probes. The accuracy gap above is therefore confounded by empty retrieval, not a retrieval-quality difference. The coding accuracy of 0.2000 [tier stub, 20 probes; not quotable] comes only from the four adversarial-abstention probes, where the abstention answer matched an empty gold answer while the context was empty.

## Constant status

Every constant below is an `unmeasured starting point`. None is justified by a tier-2 measurement, so none may be quoted as a measured result. Each value was checked in the public source before it was written here.

- Retention curve half-life: `retention.half_life_days` is `30.0` (`DEFAULT_RETENTION_HALF_LIFE_DAYS` in `crates/clio-config/src/ranking.rs`). Unmeasured starting point.
- Retention floor: `retention_floor` defaults to `0.25` (`DEFAULT_RETENTION_FLOOR` in `crates/clio-config/src/retention.rs`). The companion profile sets `0.5` and the coding profile sets `0.1`. Unmeasured starting point.
- Trust ratios: `user_stated` `1.0`, `third_party` `0.95`, `agent_inferred` `0.85` (`DEFAULT_TRUST_USER_STATED`, `DEFAULT_TRUST_THIRD_PARTY`, `DEFAULT_TRUST_AGENT_INFERRED` in `ranking.rs`). Unmeasured starting point.
- Maintenance trigger thresholds: the dirty-ancestor, eligible-hub, and stale-unit thresholds are each `1` (`DEFAULT_MAINTENANCE_TRIGGER_THRESHOLD` in `ranking.rs`). Unmeasured starting point.
- Maintenance cooldown: `300` seconds (`DEFAULT_MAINTENANCE_COOLDOWN_SECONDS` in `ranking.rs`). Unmeasured starting point.
- Maintenance hourly cap: `12` waves per hour (`DEFAULT_MAINTENANCE_MAX_WAVES_PER_HOUR` in `ranking.rs`). Unmeasured starting point.
- Maintenance hub budget: `16` hubs per wave (`DEFAULT_MAINTENANCE_HUB_BUDGET_PER_WAVE` in `ranking.rs`). Unmeasured starting point.
- Pack fidelity default: `flow` (`PackFidelityDefault::Flow`, the default in `crates/clio-config/src/retention_levels.rs`). Unmeasured starting point.
- Provenance floor default: none (`None`, the default of `provenance_floor_default` in `retention.rs`). Unmeasured starting point.
- Exact-channel bounded-render cap: `512` bytes (`EXACT_RENDER_MAX_BYTES` in `crates/clio-retrieve/src/render.rs`). Unmeasured starting point.

## Decision rule

The decision rule was applied as written before any number was read: a weak knowledge-update result points at the supersession check, the `inspect` tool's supersession section, and not at a weight. This locomo canary run has no knowledge-update category, so there is no number for that branch. The rule stands for the next run that covers a knowledge-update category.

## Inventory beside the scores

The last surviving bank held 509 items under the companion profile and 508 under the coding profile. In both banks every item is `by_tier` `semantic` and has provenance class `fact`, so the other classes are zero; the same items are also category `output_constraint` and epistemic kind `fact`. The adapter resets the database per session, so only the last session's bank is visible, not the whole run.

## Known limitations

- There is no quotable baseline, because both runs are tier `stub`.
- The comparison is a fact about this build at this commit on this machine.
- A canary run with 20 probes judged by the stub is a smoke signal.
- Both profiles run the 30-day default half-life, because no durable profile surface sets it.
- The fingerprint blind spot and the trigger-state gap above limit what the runs can show.
- No constant was changed, so this produces knowledge, not an improvement.
