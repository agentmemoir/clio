# Deployment profiles

These two files are complete retention profile definitions. Each file names all eleven required retention-profile fields explicitly (`verbosity`, `theta_offsets`, `duplicate_tolerance_write`, `duplicate_tolerance_read`, `recall_scope_default`, `retention_curve`, `retention_floor`, `pack_fidelity_default`, `provenance_floor_default`, `maintenance_trigger`, and `version`), so loading one cannot leave a required setting at a machine default, and the optional `mission` field is deliberately absent, so these profiles carry no mission. Load either file with the existing command `clio retention set --profiles <store> --scope deployment --profile-file <file>` (or set `CLIO_RETENTION_PROFILES` to `<store>`), where `<store>` is the durable retention store path. These files add no new preset mechanism.

Both profiles carry the same label: **unmeasured starting point**. No value in either file is tuned. Every value is a chosen starting point to test, not a measured result.

## `long-lived-companion.json`

This profile is for a long-lived companion, such as a personal assistant that runs for years. It assumes a session that lasts months or years, and that old turns stay useful. It sets `verbosity` to `balanced`, which keeps the admission threshold at its base value. It sets `retention_floor` to `0.5`, so the retention curve never scores a kept item below half of its value, and it sets `provenance_floor_default` to `null`, so no provenance rung is hidden. It does not set the half-life, because this build gives the profile no half-life field, so it runs the system default of 30 days; the goal of a slow half-life is recorded here as intent only, not applied.

## `coding-agent.json`

This profile is for a coding agent, such as a tool that helps write and fix code. It assumes a session that lasts hours or days, and that only a verified few facts should survive. It sets `verbosity` to `selective`, which raises the admission threshold and adds a novelty floor for writes. It sets `retention_floor` to `0.1`, so the retention curve may decay an old item far more, and it sets `provenance_floor_default` to `verified`, so only the `verified` rung is visible and weaker rungs are hidden. It does not set the half-life, because this build gives the profile no half-life field, so it runs the system default of 30 days; the goal of a fast half-life is recorded here as intent only, not applied.

## The one-sentence difference

`long-lived-companion.json` uses `verbosity` `balanced`, `duplicate_tolerance_write` `lenient`, `duplicate_tolerance_read` `lenient`, `recall_scope_default` `full`, `retention_floor` `0.5`, and `provenance_floor_default` `null`, while `coding-agent.json` uses `verbosity` `selective`, `duplicate_tolerance_write` `strict`, `duplicate_tolerance_read` `strict`, `recall_scope_default` `consolidated_only`, `retention_floor` `0.1`, and `provenance_floor_default` `verified`; the other five fields are identical.

## The honest gap: half-life

Half-life is a per-deployment ranking-environment knob in this build. It has no durable profile surface: `clio ranking get` reports `retention.half_life_days` `30.0`, `clio config set ranking.*` is refused, and `clio ranking set` lasts only for its own process. Both profiles therefore run the system default half-life of 30 days. The intended slow half-life for the companion and fast half-life for the coding agent are recorded in this file as intent, not applied, and a later change is needed to apply them.
