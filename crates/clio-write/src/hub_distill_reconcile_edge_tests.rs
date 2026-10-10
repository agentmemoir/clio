// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the reconcile edge-case scenarios: a unit
//! whose whole claim set was discarded, a unit whose revision id collides, and
//! a unit that keeps refusing admission across waves.
//!
//! ## Owns
//!
//! Test-only. Never compiled into production builds.
//!
//! ## Does not own
//! - The main reconcile wave scenarios (`hub_distill_reconcile_tests`)
//! - The reconcile decision and its outcomes (`hub_distill_reconcile`)
//! - The store revision write (`clio-store`)
//!
//! ## Boundary
//!
//! Changes to what a wave must do when a unit is degenerate should normally
//! require changes here. Changes to the ordinary reconcile path should not
//! require changes here.
//!
//! Keep this module focused on the degenerate-unit wave scenarios.

use super::hub_distill_reconcile_tests::{
    BANK, HUB, REV2, UNIT, add_source, discard_item, first_wave, record_of, seeded, wave,
};
use super::hub_distill_tests::{engine as make_engine, seed_hub, semantic, store as fresh_store};
use clio_store::{AssocKind, SqliteStore, Store};
use std::sync::Arc;

/// A wave over a unit whose every claimed source was discarded leaves it stale
/// and readable, writes no revision, and reports the documented
/// `reconcile_no_neighbors` outcome. An empty rebuild would re-admit a summary
/// whose text is the hub gist followed by `related to ` and nothing else.
#[test]
fn a_unit_with_no_remaining_sources_is_not_rebuilt() {
    let store = fresh_store();
    seed_hub(&store, BANK, HUB, &["n1", "n2", "n3"], 0.15);
    let mut engine = make_engine(&store, 0.0, 2);
    engine.set_prune_after_distill(false);
    first_wave(&mut engine);

    for id in ["n1", "n2", "n3"] {
        discard_item(&store, id);
    }

    // The cascade emptied the claim set and marked the unit stale.
    let after = record_of(&store, UNIT);
    assert!(
        after.source_ids.is_empty(),
        "claim set: {:?}",
        after.source_ids
    );
    assert_eq!(after.stale_reason.as_deref(), Some("source_removed"));

    let report = wave(&engine);
    assert_eq!(
        report.reconcile_stale_kept,
        vec![HUB.to_owned()],
        "the unit must be reported as kept stale: {report:?}"
    );
    assert!(report.revised.is_empty(), "revised: {:?}", report.revised);

    // No revision row exists, and the unit is still readable with its text.
    let record = record_of(&store, UNIT);
    assert_eq!(record.revision, 1, "no revision was written");
    assert_eq!(
        record.stale_reason.as_deref(),
        Some("source_removed"),
        "stale is not cleared by an empty rebuild"
    );
    let current = store
        .get_memory_item(BANK, UNIT)
        .unwrap()
        .expect("the unit stays readable");
    assert!(
        current.gist.is_some(),
        "the prior revision's text is retained"
    );

    // A further wave is equally quiet: this state is stable, not a retry loop.
    let again = wave(&engine);
    assert!(again.revised.is_empty(), "revised: {:?}", again.revised);
    assert_eq!(record_of(&store, UNIT).revision, 1);
}

/// A unit whose next revision id is already occupied by another row does not
/// stop the wave. The affected unit is reported and left exactly as it was,
/// and every other unit in the bank still reconciles.
///
/// The squatter is reached the way a real caller reaches it: a second hub whose
/// id already ends in `:v2` distills to the unit id `hub-distill:{bank}:{hub}:v2`,
/// which is the first unit's own revision-2 id. Both are ordinary writes
/// through the ordinary surface.
#[test]
fn a_colliding_revision_id_does_not_stop_the_wave() {
    let store = fresh_store();
    // Only the squatter hub is seeded first, so its unit row takes the id
    // `hub-distill:bank:hub:v2` — which is the first unit's revision-2 id.
    // Both writes are ordinary; nothing here writes an id by hand.
    let squatter_hub = format!("{HUB}:v2");
    seed_hub(&store, BANK, &squatter_hub, &["q1", "q2", "q3"], 0.15);
    let mut squatter_engine = make_engine(&store, 0.0, 2);
    squatter_engine.set_prune_after_distill(false);
    let squatted = wave(&squatter_engine).distilled;
    assert_eq!(
        squatted,
        vec![REV2.to_owned()],
        "the squatter unit's id is the first unit's revision-2 id"
    );

    // Now seed the first hub and distill it at revision 1; the id its first
    // revision would take is already spoken for.
    seed_hub(&store, BANK, HUB, &["n1", "n2", "n3"], 0.15);
    let mut engine = make_engine(&store, 0.0, 2);
    engine.set_prune_after_distill(false);
    first_wave(&mut engine);
    // Drop the first hub below the degree limit so the reconcile pass, not the
    // eligible loop, is what reaches for a revision on the next wave.
    drop_below_limit(&store, HUB);
    add_source(&store, "n4");

    let report = wave(&engine);
    assert!(
        report.revised.is_empty(),
        "the colliding unit cannot revise: {report:?}"
    );
    assert_eq!(
        report.reconcile_stale_kept,
        vec![HUB.to_owned()],
        "it is reported, not silently dropped: {report:?}"
    );

    // The unit is untouched: no revision, prior still current, and the refused
    // revision claimed nothing.
    let record = record_of(&store, UNIT);
    assert_eq!(record.revision, 1);
    assert_eq!(
        record.source_ids,
        vec!["n1".to_owned(), "n2".to_owned(), "n3".to_owned()],
        "the refused revision changed no claim set"
    );
    let prior = store.get_memory_item(BANK, UNIT).unwrap().unwrap();
    assert_eq!(prior.tx_until, None, "the prior stays current");

    // A second unit in the same bank still reconciles: one bad id cannot stop
    // reconciliation for the rest of the bank.
    let healthy = seed_below_limit_unit(&store);
    let report = wave(&engine);
    assert_eq!(
        report.revised,
        vec!["hub2".to_owned()],
        "the healthy unit revises: {report:?}"
    );
    assert_eq!(record_of(&store, &healthy).revision, 2);
}

/// A unit that keeps refusing admission is retried each wave — deliberately,
/// so a policy change can un-stick it. A first refusal is a determination and
/// writes its own derived audit row; a retry that hits the same refusal on
/// unchanged input writes nothing at all.
#[test]
fn a_unit_that_keeps_refusing_admission_writes_nothing_on_a_second_wave() {
    let (store, mut normal_engine) = seeded();
    first_wave(&mut normal_engine);
    add_source(&store, "n4");
    // A stricter policy refuses the re-derived revision.
    let mut strict = make_engine(&store, 1.0, 2);
    strict.set_prune_after_distill(false);

    assert_eq!(
        reconcile_audit_count(&store, UNIT),
        0,
        "the first wave distils; it does not reconcile"
    );
    let first = wave(&strict);
    assert_eq!(
        first.reconcile_rejected,
        vec![HUB.to_owned()],
        "the strict policy refuses: {first:?}"
    );
    let after_first = record_of(&store, UNIT);
    assert_eq!(
        after_first.stale_reason.as_deref(),
        Some("revision_rejected")
    );
    // A refusal is a determination like a pass, and must leave the same kind of
    // durable trace: the engine's default sink discards the gate's own
    // telemetry, so this row is the only record of why the unit went stale.
    assert_eq!(
        reconcile_audit_admitted(&store, UNIT),
        vec![false],
        "a first refusal is audited as a non-admitted determination"
    );

    // A second wave over the same refusal writes nothing new.
    let before_ops = reconcile_audit_count(&store, UNIT);
    let second = wave(&strict);
    assert_eq!(
        second.reconcile_rejected,
        vec![HUB.to_owned()],
        "it is still reported as refused: {second:?}"
    );
    let after_second = record_of(&store, UNIT);
    // The refusal detail carries the admission score, which is not a stable
    // function of the input, so it is deliberately not compared. Everything
    // that says "nothing moved" is.
    assert_eq!(
        (
            after_second.revision,
            after_second.source_ids,
            after_second.stale_reason,
            after_second.stale_at,
        ),
        (
            after_first.revision,
            after_first.source_ids,
            after_first.stale_reason,
            after_first.stale_at,
        ),
        "an unchanged refusal must not rewrite the record"
    );
    assert_eq!(
        reconcile_audit_count(&store, UNIT),
        before_ops,
        "an unchanged refusal must not write another audit row"
    );

    // A policy change that now admits the candidate still revises the unit.
    let third = wave(&normal_engine);
    assert_eq!(
        third.revised,
        vec![HUB.to_owned()],
        "a permissive policy un-sticks it: {third:?}"
    );
    let record = record_of(&store, UNIT);
    assert_eq!(record.revision, 2);
    assert_eq!(record.stale_reason, None);
}

/// Seed a second unit and drop its hub below the degree limit, so the reconcile
/// pass — not the eligible loop — is what revises it in the later wave.
fn seed_below_limit_unit(store: &Arc<SqliteStore>) -> String {
    let hub2 = "hub2";
    seed_hub(store, BANK, hub2, &["m1", "m2", "m3"], 0.15);
    let mut second = make_engine(store, 0.0, 2);
    second.set_prune_after_distill(false);
    assert_eq!(
        wave(&second).distilled,
        vec![format!("hub-distill:{BANK}:{hub2}")]
    );
    drop_below_limit(store, hub2);
    // A later neighbour is what the reconcile pass will pick up.
    store
        .create_memory_item(&semantic("m4", BANK, "neighbor m4"), "agent")
        .unwrap();
    store
        .reinforce_assoc_edge(BANK, hub2, "m4", 0, 0.15)
        .expect("reinforce");
    format!("hub-distill:{BANK}:{hub2}")
}

/// Drop a hub's coactivation edges so its degree collapses and it is no longer
/// an eligible hub: the reconcile pass, not the eligible loop, will visit its
/// unit on the next wave.
fn drop_below_limit(store: &Arc<SqliteStore>, hub: &str) {
    for edge in store.list_assoc_edges(BANK, Some(hub)).unwrap() {
        if edge.assoc_kind == AssocKind::Coactivation {
            store.delete_assoc_edge(BANK, &edge.id).unwrap();
        }
    }
}

/// How many reconciliation audit rows the unit has, which is the observable
/// side effect an unchanged refusal must not add to.
fn reconcile_audit_count(store: &Arc<SqliteStore>, unit: &str) -> usize {
    reconcile_audit_admitted(store, unit).len()
}

/// The `admitted` flag of each reconciliation audit row, in trail order: the
/// durable record of whether that distillation event produced a revision.
fn reconcile_audit_admitted(store: &Arc<SqliteStore>, unit: &str) -> Vec<bool> {
    store
        .audit_trail(BANK, unit)
        .unwrap()
        .events
        .iter()
        .filter(|e| e.operation == "hub_distill_reconcile")
        .map(|e| {
            e.detail
                .get("admitted")
                .and_then(serde_json::Value::as_bool)
                .expect("a reconcile audit row records admitted")
        })
        .collect()
}
