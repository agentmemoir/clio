// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the end-to-end evidence that a maintenance
//! wave the CLI reports actually ran, that the live ranking environment governs
//! hub eligibility inside one process, and that both bindings produce the same
//! distillation for the same bank.
//!
//! ## Owns
//! - The wave scenarios in this file, each asserting on stored artifacts (audit
//!   rows, distilled item rows) rather than on internal counters
//!
//! ## Does not own
//! - The fixture, the `clio` launcher, and the MCP session
//!   (`consolidate_wave_support`)
//! - The in-process `consolidate_tool` and `HubDistillEngine` unit tests
//!   (`clio-write`'s `hub_distill_tests`) and the ranking-plumbing tests
//!   (`clio-mcp`'s `runtime_ranking_tests`)
//! - Per-knob decay arithmetic, which needs an injected clock and therefore
//!   belongs in `clio-write`'s `hub_distill_policy_tests`
//!
//! ## Boundary
//!
//! Changes to how a binding reports or performs a `consolidate` wave should
//! normally require changes here. Changes to the hub algorithm itself should
//! not require changes here.
//!
//! Keep this module focused on wave completion and cross-binding parity.

mod common;
#[path = "consolidate_wave_support/mod.rs"]
mod consolidate_wave_support;

use consolidate_wave_support::{Fixture, HUB_ITEMS, McpSession, TUNABLE_ITEMS, distilled_count};
use serde_json::Value;

/// The co-activation degree `n` fully connected items produce: each one links
/// to the other `n - 1`.
fn degree_of(items: usize) -> i64 {
    i64::try_from(items).expect("item count fits") - 1
}

#[test]
fn cli_consolidate_reports_the_wave_it_actually_ran() {
    let f = Fixture::new();
    f.seed_coactivated_bank(HUB_ITEMS);
    assert_eq!(f.max_coactivation_degree(), degree_of(HUB_ITEMS));

    // The whole point: the process exits right after this, so the work has to be
    // finished by the time the command returns.
    let payload = f.json(&["consolidate", "--force", "--output", "json"]);

    assert_eq!(payload["ok"], Value::Bool(true), "{payload}");
    assert_eq!(
        payload["status"],
        Value::String("completed".into()),
        "{payload}"
    );
    let summary = payload["hubs"].as_str().expect("hubs summary");
    assert!(summary.starts_with("hub_distill:"), "hubs: {summary}");

    // What the command reported must match what the store kept.
    let ids = f.distilled_ids();
    assert_eq!(ids.len(), HUB_ITEMS, "every seeded node is above the limit");
    assert_eq!(
        distilled_count(summary),
        Some(u32::try_from(ids.len()).expect("count fits")),
        "reported {summary} but stored {ids:?}"
    );
    let audit = f.hub_audit();
    assert!(
        audit.contains(&("hub_distill_complete".to_owned(), 1)),
        "the wave must leave a completion record: {audit:?}"
    );
    assert_eq!(
        audit
            .iter()
            .find(|(op, _)| op == "hub_distill")
            .map(|(_, n)| *n),
        Some(i64::try_from(ids.len()).expect("count fits")),
        "each distilled item needs its own derived audit record: {audit:?}"
    );

    // The stored artifacts must carry the recall contract and a derivation
    // record. This process is not the one that ran the wave, so the rows it
    // reads are also the record's durability evidence.
    let records = f.distilled_records();
    assert_eq!(
        records.len(),
        ids.len(),
        "one derivation record per stored unit: {records:?}"
    );
    // Every claimed source must be a stored item in the same bank.
    let stored = f.stored_item_ids();
    for (item_id, source_ref, hub_id, source_ids_json) in &records {
        assert_eq!(
            source_ref,
            clio_retrieve::HUB_DISTILL_SOURCE_REF,
            "the stored provenance marker must be the one the predicate matches: {item_id}"
        );
        assert!(
            item_id.ends_with(&format!(":{hub_id}")),
            "the record must be keyed by the unit id and name its hub: {item_id} -> {hub_id:?}"
        );
        let claimed: Vec<String> =
            serde_json::from_str(source_ids_json).expect("claim set is a JSON array");
        let mut unique = claimed.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(
            unique.len(),
            claimed.len(),
            "no source id repeats: {item_id}"
        );
        assert!(
            !claimed.is_empty(),
            "a distilled unit must claim its sources: {item_id}"
        );
        // The store does not forbid a self edge today, so the raw source list may
        // name the hub. The record guarantees the hub by its own column, and
        // its claim view is the source ids plus the hub, so the assertions
        // here pin the deduplicated claim set rather than the raw list.
        assert!(
            !hub_id.is_empty(),
            "the record must name its hub: {item_id}"
        );
        for source in &claimed {
            assert!(
                stored.contains(source),
                "a claimed source must be a stored item in the same bank: {item_id} -> {source}"
            );
        }
    }
}

/// A wave after a compliance erasure must still run: erasing a quoted source's
/// subject purges the claiming unit's content, and the next wave reports the
/// purged unit instead of failing on its unreadable row.
#[test]
fn a_wave_after_an_erasure_runs_and_reports_the_purged_unit() {
    let f = Fixture::new();
    f.seed_coactivated_bank(HUB_ITEMS);
    f.json(&["consolidate", "--force", "--output", "json"]);
    assert_eq!(f.distilled_ids().len(), HUB_ITEMS);

    // Erase one quoted source's subject. CLI-seeded items each own their
    // subject, so this purges only the units quoting that one item.
    let source = erased_source_subject(&f);
    f.assert_ok(&[
        "erase",
        "--subject",
        &source,
        "--basis",
        "data-subject request",
        "--request",
        "req-e2e-1",
        "--confirm",
        "--actor",
        "harness",
        "--output",
        "json",
    ]);

    // The wave survives the purged units and reports them.
    let payload = f.json(&["consolidate", "--force", "--output", "json"]);
    assert_eq!(payload["ok"], Value::Bool(true), "{payload}");
    let summary = payload["hubs"].as_str().expect("hubs summary");
    // Every unit quoting the erased source purged (the derivation closure);
    // the wave reports them as stale-kept instead of failing, and the units
    // that did not quote it stay current.
    let stale_kept = summary
        .split("stale_kept=")
        .nth(1)
        .and_then(|rest| rest.split(',').next())
        .and_then(|n| n.parse::<u32>().ok())
        .expect("stale_kept count in the summary: {summary}");
    assert!(stale_kept >= 1, "the purged unit is reported: {summary}");
    assert!(
        summary.contains("unchanged="),
        "the remaining units are unchanged: {summary}"
    );
}

/// The subject of one raw item a distilled unit quotes (CLI items own their
/// subjects), read from the store so the test does not re-derive ids.
fn erased_source_subject(f: &Fixture) -> String {
    let record = &f.distilled_records()[0];
    let claimed: Vec<String> = serde_json::from_str(&record.3).expect("claim set is a JSON array");
    let source = claimed
        .iter()
        .find(|id| !id.starts_with("hub-distill:"))
        .expect("a raw source id");
    f.rows(&format!(
        "select coalesce(subject_id, id), 0 from items where id = '{}'",
        source.replace('\'', "''")
    ))
    .remove(0)
    .0
}

#[test]
fn a_fresh_process_does_not_honor_another_process_ranking_change() {
    // One prepared bank, copied so each process sees the same starting state.
    let prepared = Fixture::new();
    prepared.seed_coactivated_bank(TUNABLE_ITEMS);
    assert_eq!(prepared.max_coactivation_degree(), degree_of(TUNABLE_ITEMS));

    let tuned = prepared.copy_of();
    let mut session = McpSession::open(&tuned);
    let applied = session.call(
        2,
        "ranking_env_set",
        serde_json::json!({"patch": {"coactivation": {
            "eta": 0.15, "half_life_days": 30.0,
            "hub_degree_limit": 1, "max_hops": 2, "w_min": 0.05
        }}}),
    );
    assert_eq!(applied["applied"], Value::Bool(true), "{applied}");
    let readback = session.call(3, "ranking_env_get", serde_json::json!({}));
    assert_eq!(
        readback["coactivation"]["hub_degree_limit"],
        Value::from(1),
        "{readback}"
    );
    let queued = session.call(4, "consolidate", serde_json::json!({}));
    assert_eq!(
        queued["status"],
        Value::String("accepted".into()),
        "{queued}"
    );
    tuned.await_hub_wave();
    assert_eq!(
        tuned.distilled_ids().len(),
        TUNABLE_ITEMS,
        "the tuned limit should admit every node"
    );
    assert!(
        session.finish().success(),
        "the MCP session must exit cleanly"
    );

    // A second process on the identical bank starts from the effective
    // defaults, where degree 5 is below the limit of 16, so nothing distils.
    let fresh = prepared.copy_of();
    let payload = fresh.json(&["consolidate", "--force", "--output", "json"]);
    assert_eq!(
        payload["status"],
        Value::String("completed".into()),
        "{payload}"
    );
    assert_eq!(
        distilled_count(payload["hubs"].as_str().unwrap_or_default()),
        Some(0)
    );
    assert!(
        fresh.distilled_ids().is_empty(),
        "the change is session-scoped, not persisted"
    );
}

/// A second knob, driven through the same published tool, has to reach the same
/// scan. `w_min` is the prune threshold: set above every coactivation weight in
/// the bank and no edge counts toward any degree, so the wave finds no hubs even
/// though the degree limit now admits every node. The companion test above sets
/// the same limit and leaves `w_min` at its default, and distils all of them, so
/// the two runs differ only in this knob.
#[test]
fn a_raised_w_min_through_ranking_env_set_empties_the_wave() {
    let prepared = Fixture::new();
    prepared.seed_coactivated_bank(TUNABLE_ITEMS);
    let f = prepared.copy_of();
    let mut session = McpSession::open(&f);
    let applied = session.call(
        2,
        "ranking_env_set",
        serde_json::json!({"patch": {"coactivation": {
            "eta": 0.15, "half_life_days": 30.0,
            "hub_degree_limit": 1, "max_hops": 2, "w_min": 1.0
        }}}),
    );
    assert_eq!(applied["applied"], Value::Bool(true), "{applied}");
    let queued = session.call(3, "consolidate", serde_json::json!({}));
    assert_eq!(
        queued["status"],
        Value::String("accepted".into()),
        "{queued}"
    );
    let audit = f.await_hub_wave();
    assert!(
        audit.contains(&("hub_distill_complete".to_owned(), 1)),
        "the wave must still run and record itself: {audit:?}"
    );
    assert!(
        f.distilled_ids().is_empty(),
        "no edge clears a prune threshold of 1.0, so nothing is a hub"
    );
    assert!(
        session.finish().success(),
        "the MCP session must exit cleanly"
    );
}

#[test]
fn cli_and_mcp_produce_the_same_distillation_for_the_same_bank() {
    // Both bindings get a byte-identical copy of one prepared bank.
    let prepared = Fixture::new();
    prepared.seed_coactivated_bank(HUB_ITEMS);

    let cli = prepared.copy_of();
    let cli_payload = cli.json(&["consolidate", "--force", "--output", "json"]);
    assert_eq!(
        cli_payload["status"],
        Value::String("completed".into()),
        "{cli_payload}"
    );

    let mcp = prepared.copy_of();
    let mut session = McpSession::open(&mcp);
    let queued = session.call(2, "consolidate", serde_json::json!({"force": true}));
    assert_eq!(
        queued["status"],
        Value::String("accepted".into()),
        "{queued}"
    );
    mcp.await_hub_wave();
    assert!(
        session.finish().success(),
        "the MCP session must exit cleanly"
    );

    let cli_summary = cli_payload["hubs"].as_str().expect("cli hubs");
    assert_eq!(
        cli.distilled_ids(),
        mcp.distilled_ids(),
        "the same bank must distil to the same item ids on both bindings"
    );
    assert_eq!(
        distilled_count(cli_summary),
        Some(u32::try_from(cli.distilled_ids().len()).expect("count fits")),
        "the CLI reported {cli_summary}"
    );
    // The MCP binding answers before its wave runs, so its own hub set is only
    // observable through the stored artifacts. Those are the same records the
    // CLI's joined wave wrote, so the audit rows must match too.
    assert_eq!(cli.hub_audit(), mcp.hub_audit());
}

/// A wave that cannot run must not be reported as a completed one.
///
/// The store here is a database file with no key store beside it, the shape a
/// restore that lost its key store has: it opens, then fails at the first
/// encrypted read. The CLI joined the wave, so it owns that failure and must
/// report it rather than return a success envelope over missing work.
#[test]
fn a_wave_that_cannot_run_reports_a_failure_not_a_completed_wave() {
    let prepared = Fixture::new();
    prepared.seed_coactivated_bank(HUB_ITEMS);
    let broken = prepared.copy_without_keystore();

    let out = broken.output(&["consolidate", "--force", "--output", "json"]);
    assert!(
        !out.status.success(),
        "a wave that could not run must not exit zero: {out:?}"
    );
    assert!(
        out.stdout.is_empty(),
        "a failed command has no success payload, got: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    let payload = consolidate_wave_support::json_stderr(&out);
    assert_eq!(payload["ok"], Value::Bool(false), "{payload}");
    assert_eq!(
        payload["code"],
        Value::String("forbidden".into()),
        "the failure must carry the code the wave hit: {payload}"
    );
    assert!(
        payload["status"].is_null() && payload["hubs"].is_null(),
        "a failed wave must not report a status or a hub summary: {payload}"
    );
    assert!(
        broken.distilled_ids().is_empty(),
        "nothing should be written"
    );
}

#[test]
fn consolidate_on_an_empty_bank_reports_no_hubs_and_writes_none() {
    let f = Fixture::new();
    let payload = f.json(&["consolidate", "--force", "--output", "json"]);
    assert_eq!(payload["ok"], Value::Bool(true), "{payload}");
    assert_eq!(
        payload["status"],
        Value::String("completed".into()),
        "{payload}"
    );
    assert_eq!(
        distilled_count(payload["hubs"].as_str().unwrap_or_default()),
        Some(0)
    );
    assert!(f.distilled_ids().is_empty(), "no distilled items expected");
}
