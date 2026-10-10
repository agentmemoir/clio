// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the pressure-witness integration tests: a
//! `maintenance_status` pressure read reaches no mutating `Store` method and
//! no writer behind any wrapped view handle, starts no maintenance wave, keeps
//! stored state as its stale-unit source, answers with an unavailable entry
//! when that source fails, and adds `pressure` as the only new top-level key.
//!
//! ## Owns
//! - The scenario tests below and the legacy-field comparison they run
//!
//! ## Does not own
//! - The counting double and its writer-name derivation (`store_double`)
//! - The double installation, the view doubles, and the dummy arguments (`main`)
//! - The pressure assembly (`clio-mcp` pressure module)
//! - The shared read fixture state (`mcp_read_support`)
//!
//! ## Boundary
//!
//! Changes to what a pressure read reaches should normally require changes
//! here. Changes to the pressure shape itself should not.
//!
//! Keep this module focused on witnessing the read path.

use crate::store_double::{CountingStore, WRITER_NAMES, derived_writer_names};
use crate::{
    FIXTURE_BANK, dummy_correction, dummy_edge, dummy_item, dummy_node, dummy_record, dummy_slot,
    dummy_triple, seeded_state, status_call, support, with_double,
};
use clio_store::{DistillIntent, DistilledUnitStore, Store};
use serde_json::{Value, json};
use std::sync::Arc;

/// Lifecycle timestamps one tick after the dummies' `FIXTURE_TS`, as the
/// writer-proof lifecycle writers take them.
const TS: &str = "2026-01-02T00:00:00Z";

/// The claim stamp the writer-proof claim carries, in the millisecond-epoch
/// text a derivation record's `distilled_at` uses. A literal, never the clock.
const CLAIMED_AT: &str = "1780272000000";

/// A pressure read reaches no mutating store method and starts no wave. The
/// cumulative wave-start counter is sampled before and after the call, so a
/// wave that started and finished inside the call still fails the test; the
/// view counter proves no writer behind any wrapped handle ran either.
#[test]
fn pressure_read_reaches_no_writer_and_starts_no_wave() {
    let seeded = seeded_state();
    let double = Arc::new(CountingStore::new(Arc::clone(&seeded.store)));
    let state = with_double(&seeded, &double);

    let waves_before = state.maint.waves_started();
    let out = status_call(&state);

    assert_eq!(out["ok"], json!(true), "{out}");
    assert_eq!(
        double.mutations(),
        0,
        "a pressure read must not reach a mutating store method: {out}"
    );
    assert_eq!(
        double.view_writes(),
        0,
        "a pressure read must not reach a writer behind any wrapped view handle: {out}"
    );
    assert_eq!(
        state.maint.waves_started(),
        waves_before,
        "a pressure read must not start a maintenance wave: {out}"
    );
}

/// With no wave in this process, the stale count comes from the stored unit,
/// names its source, and reports no wave snapshot.
#[test]
fn stored_stale_state_names_its_source_without_a_wave() {
    let seeded = seeded_state();
    let double = Arc::new(CountingStore::new(Arc::clone(&seeded.store)));
    let state = with_double(&seeded, &double);

    let out = status_call(&state);

    assert_eq!(out["ok"], json!(true), "{out}");
    let stale_units = &out["pressure"]["stale_units"];
    assert_eq!(stale_units["source"], json!("stored_state"), "{out}");
    assert_eq!(stale_units["count"], json!(1), "{out}");
    assert_eq!(stale_units["last_wave"], Value::Null, "{out}");
}

/// The witness is live. Without this proof, a double that counts nothing would satisfy every zero assertion above. One write through the store double and one through a wrapped view handle each move their counter.
#[test]
fn a_known_write_through_the_double_increments_the_counter() {
    let seeded = seeded_state();
    let double = Arc::new(CountingStore::new(Arc::clone(&seeded.store)));
    let state = with_double(&seeded, &double);

    let before = double.mutations();
    state
        .store
        .record_audit_event(
            support::BANK,
            "witness_live",
            None,
            "agent",
            &json!({"probe": true}),
        )
        .expect("known write through the double");
    assert_eq!(
        double.mutations(),
        before + 1,
        "the counting double must count a real write"
    );

    let views_before = double.view_writes();
    state
        .ops_store
        .drop_orphan_assoc_edges(support::BANK)
        .expect("known write through the wrapped ops view");
    assert_eq!(
        double.view_writes(),
        views_before + 1,
        "the view double must count a real write"
    );
}

/// Run one writer call, require its name in `WRITER_NAMES`, and require the
/// double to count exactly the one mutation the running expected.
fn counted_write<R>(d: &CountingStore, n: &mut usize, label: &str, call: impl FnOnce() -> R) {
    let _ = call();
    *n += 1;
    assert!(
        WRITER_NAMES.contains(&label),
        "`{label}` must be named in WRITER_NAMES"
    );
    assert_eq!(d.mutations(), *n, "`{label}` must be counted by the double");
}

/// Every store method documented as writing is wrapped and counted, and the
/// hand-written writer list equals the set derived from the trait sources. The
/// four whose names do not begin with a write verb are called here like the
/// other eighteen.
#[test]
fn every_documented_writer_is_wrapped_and_counted() {
    let seeded = seeded_state();
    let double = Arc::new(CountingStore::new(Arc::clone(&seeded.store)));
    let bank = support::BANK;
    let mut expected = 0usize;

    exercise_item_writers(&double, bank, &mut expected);
    exercise_assoc_and_audit_writers(&double, bank, &mut expected);
    exercise_vector_and_memtree_writers(&double, bank, &mut expected);
    exercise_triple_and_lifecycle_writers(&double, bank, &mut expected);
    exercise_distilled_unit_writers(&double, &mut expected);

    for odd in [
        "reinforce_assoc_edge",
        "invalidate_item",
        "correct_item",
        "discard_item",
    ] {
        assert!(WRITER_NAMES.contains(&odd), "{odd} must be in the list");
    }
    let mut listed = WRITER_NAMES.to_vec();
    listed.sort_unstable();
    let mut unique = listed.clone();
    unique.dedup();
    assert_eq!(
        unique.len(),
        listed.len(),
        "WRITER_NAMES must not repeat a name"
    );
    assert_eq!(
        listed,
        derived_writer_names(),
        "WRITER_NAMES must equal the writer set derived from the trait doc comments"
    );
    assert_eq!(
        double.mutations(),
        WRITER_NAMES.len(),
        "one count per named writer, and no writer outside the list"
    );
}

/// Call every item writer once in trait order, counting exactly one each.
fn exercise_item_writers(d: &CountingStore, bank: &str, n: &mut usize) {
    counted_write(d, n, "delete_item", || d.delete_item(bank, "missing"));
    counted_write(d, n, "create_memory_item", || {
        d.create_memory_item(&dummy_item("w-create"), "agent")
    });
    counted_write(d, n, "update_memory_item", || {
        d.update_memory_item(&dummy_item("w-update"), "agent")
    });
}

/// Call every association and audit writer once in trait order, counting once.
fn exercise_assoc_and_audit_writers(d: &CountingStore, bank: &str, n: &mut usize) {
    let detail = json!({});
    counted_write(d, n, "put_assoc_edge", || d.put_assoc_edge(&dummy_edge()));
    counted_write(d, n, "delete_assoc_edge", || {
        d.delete_assoc_edge(bank, "missing")
    });
    counted_write(d, n, "reinforce_assoc_edge", || {
        d.reinforce_assoc_edge(bank, "a", "b", 0, 0.5)
    });
    counted_write(d, n, "record_assoc_audit", || {
        d.record_assoc_audit(bank, "probe", None, "agent", &detail)
    });
    counted_write(d, n, "record_audit_event", || {
        d.record_audit_event(bank, "probe", None, "agent", &detail)
    });
}

/// Call every vector and `MemTree` writer once in trait order, counting once.
fn exercise_vector_and_memtree_writers(d: &CountingStore, bank: &str, n: &mut usize) {
    counted_write(d, n, "upsert_vector", || d.upsert_vector(&dummy_slot()));
    counted_write(d, n, "delete_vector", || d.delete_vector(bank, "missing"));
    counted_write(d, n, "upsert_subject_key_ref", || {
        d.upsert_subject_key_ref("subject", "dek-ref", "active")
    });
    counted_write(d, n, "upsert_memtree_node", || {
        d.upsert_memtree_node(&dummy_node())
    });
}

/// Call every triple and lifecycle writer once in trait order, counting once.
fn exercise_triple_and_lifecycle_writers(d: &CountingStore, bank: &str, n: &mut usize) {
    let item = dummy_item("w-triple");
    let triple = dummy_triple();
    counted_write(d, n, "commit_triple_add", || {
        d.commit_triple_add(&item, &triple, false, "agent")
    });
    counted_write(d, n, "triple_end", || {
        d.triple_end(bank, "s", "p", None, TS, TS)
    });
    counted_write(d, n, "invalidate_item", || {
        d.invalidate_item(bank, "missing", TS, TS, None, "agent")
    });
    counted_write(d, n, "discard_item", || {
        d.discard_item(bank, "missing", "probe", TS, "agent")
    });
    counted_write(d, n, "correct_item", || {
        d.correct_item(&dummy_correction(dummy_item("w-correct")))
    });
}

/// Call every distilled-unit writer once in trait order, counting once.
fn exercise_distilled_unit_writers(d: &CountingStore, n: &mut usize) {
    let item = dummy_item("w-unit");
    let record = dummy_record("w-unit");
    counted_write(d, n, "commit_distilled_unit", || {
        d.commit_distilled_unit(&item, &record, "agent")
    });
    let record = dummy_record("w-revise");
    let item = dummy_item("w-revise");
    counted_write(d, n, "revise_distilled_unit", || {
        d.revise_distilled_unit(&record, &item, "agent")
    });
    let record = dummy_record("w-update-unit");
    counted_write(d, n, "update_distilled_unit", || {
        d.update_distilled_unit(&record, "agent")
    });
    let exp = dummy_record("w-cas");
    let upd = dummy_record("w-cas");
    counted_write(d, n, "update_distilled_unit_if_unchanged", || {
        d.update_distilled_unit_if_unchanged(&exp, &upd, "agent")
    });
    let hub = "w-intent-hub";
    let intent = DistillIntent::new(
        FIXTURE_BANK,
        hub,
        format!("hub-distill:{FIXTURE_BANK}:{hub}"),
        "agent",
        CLAIMED_AT,
    );
    // The claim must land, or the completion below would name no claim and the
    // pair would be two calls that never wrote anything.
    counted_write(d, n, "write_distill_intent", || {
        assert!(d.write_distill_intent(FIXTURE_BANK, &intent).unwrap());
    });
    counted_write(d, n, "complete_distill_intent", || {
        assert!(
            d.complete_distill_intent(FIXTURE_BANK, hub, "w-intent-revision")
                .unwrap()
        );
    });
}

/// One source failing turns into an unavailable reason; the call still
/// succeeds and every other component still reports.
#[test]
fn a_failing_stale_source_reports_unavailable_and_still_answers() {
    let seeded = seeded_state();
    let double = Arc::new(CountingStore::with_list_distilled_units_fault(
        Arc::clone(&seeded.store),
        true,
    ));
    let state = with_double(&seeded, &double);

    let out = status_call(&state);

    assert_eq!(out["ok"], json!(true), "{out}");
    let pressure = out["pressure"].as_object().expect("pressure object");
    let stale_units = &pressure["stale_units"];
    let reason = stale_units["unavailable"]
        .as_str()
        .expect("unavailable reason on the failing source");
    assert!(
        reason.contains("stored stale state read failed"),
        "{reason}"
    );
    assert!(reason.contains("internal"), "{reason}");
    assert!(
        stale_units.get("count").is_none(),
        "no fabricated zero: {out}"
    );
    for (name, entry) in pressure {
        if name == "stale_units" {
            continue;
        }
        assert!(
            entry.get("count").is_some(),
            "{name} must still report its count: {entry}"
        );
    }
}

/// `pressure` and `trigger` are the only added top-level keys. Everything
/// else is exactly what the pre-existing status APIs return for the same
/// state.
#[test]
fn pressure_is_the_only_added_top_level_key() {
    let seeded = seeded_state();
    let double = Arc::new(CountingStore::new(Arc::clone(&seeded.store)));
    let state = with_double(&seeded, &double);

    let out = status_call(&state);
    assert_eq!(out["ok"], json!(true), "{out}");

    let mut expected = serde_json::to_value(clio_write::maintenance_status(
        &state.maint,
        support::BANK,
        None,
    ))
    .expect("maintenance status serializes");
    expected["index"] = state
        .index_status_json(support::BANK)
        .expect("index status serializes");
    let mut actual = out.clone();
    let status_object = actual.as_object_mut().expect("status object");
    let pressure = status_object.remove("pressure");
    assert!(pressure.is_some(), "pressure must be present: {out}");
    let trigger = status_object.remove("trigger");
    assert!(trigger.is_some(), "trigger must be present: {out}");
    assert_eq!(
        actual, expected,
        "every pre-existing response field must be unchanged"
    );
}

/// Secondary witness: a pressure read leaves every public stored-state read
/// unchanged. Equal fingerprints do not prove the absence of a write, because
/// a write can bypass every read observed here; the counting double is the
/// primary proof, and this equality is the supporting check.
#[test]
fn pressure_read_leaves_every_state_read_unchanged() {
    let seeded = seeded_state();
    let double = Arc::new(CountingStore::new(Arc::clone(&seeded.store)));
    let state = with_double(&seeded, &double);

    let fingerprint = |state: &clio_mcp::runtime::McpState| {
        (
            state.ops_store.counts().expect("ops counts"),
            state
                .store
                .list_distilled_units(support::BANK)
                .expect("distilled units"),
            state.maint.waves_started(),
            state
                .index_drain
                .status(Some(support::BANK))
                .expect("index status"),
        )
    };

    let before = fingerprint(&state);
    let out = status_call(&state);
    assert_eq!(out["ok"], json!(true), "{out}");
    let after = fingerprint(&state);

    assert_eq!(
        before, after,
        "the pressure read must leave every stored-state read unchanged: \
         before={before:?} after={after:?}"
    );
}

/// The same state read twice with the same explicit timestamp yields
/// identical pressure. The legacy fields are stable too because every input
/// is pinned: explicit timestamp, no background wave, fixed seeded state.
#[test]
fn the_same_state_twice_produces_identical_pressure() {
    let seeded = seeded_state();
    let double = Arc::new(CountingStore::new(Arc::clone(&seeded.store)));
    let state = with_double(&seeded, &double);

    let first = status_call(&state);
    let second = status_call(&state);

    assert_eq!(first["ok"], json!(true), "{first}");
    assert_eq!(second["ok"], json!(true), "{second}");
    assert_eq!(first["pressure"], second["pressure"], "{first}\n{second}");
    assert_eq!(first, second, "every field is stable for a pinned state");
}
