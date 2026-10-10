// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the snapshot-entity projection: the one populated shape,
//! the trim-only canonicalization, and every documented empty-coverage case.
//!
//! ## Owns
//! - `snapshot_entities` unit assertions for each write shape
//! - The empty-coverage table check (triple carrier, hub-distill, belief,
//!   snapshot-less, non-default fields) as data, not as prose
//!
//! ## Does not own
//! - Hit assembly through the retriever (`hit_entities_tests`)
//! - Binding payloads and the PII boundary (`clio-mcp` / `clio` read tests)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the entity projection unit contract.

use crate::entities::snapshot_entities;
use crate::fixtures_tests as fx;
use serde_json::{Value, json};

/// An episodic task item whose snapshot is the given value.
fn with_snapshot(snapshot: Value) -> clio_types::MemoryItem {
    let mut item = fx::item("itm-e", "bank-e", "entity fixture");
    item.snapshot = Some(snapshot);
    item
}

/// A default-field write exposes exactly the verified value.
#[test]
fn default_field_write_exposes_the_verified_snapshot_entity() {
    let item = with_snapshot(json!({"entity": "dark mode", "amount": 1, "date": "2024-01-01"}));
    assert_eq!(snapshot_entities(&item), vec!["dark mode".to_owned()]);
}

/// surrounding whitespace is removed and nothing else changes.
///
/// The value keeps its case, internal spacing, and punctuation, because the
/// write-side span matcher verified this exact text as a source substring.
#[test]
fn entity_is_trimmed_and_nothing_else_is_rewritten() {
    let item = with_snapshot(json!({"entity": "  Ada Lovelace  "}));
    assert_eq!(snapshot_entities(&item), vec!["Ada Lovelace".to_owned()]);

    let preserved = with_snapshot(json!({"entity": "ACME  Widget (v2)"}));
    assert_eq!(
        snapshot_entities(&preserved),
        vec!["ACME  Widget (v2)".to_owned()],
        "inner spacing, case, and punctuation survive verbatim"
    );
}

/// Trim-only means trim-only: a value stored in a decomposed Unicode form comes
/// back as stored, not re-normalized to NFC. The write-side matcher compared the
/// NFC form against the source, so this pins the read side's documented choice
/// rather than the matcher's.
#[test]
fn a_decomposed_entity_is_exposed_as_stored_not_re_normalized() {
    let decomposed = "Cafe\u{301} Noir";
    assert_ne!(decomposed, "Caf\u{e9} Noir", "the fixture is decomposed");
    let item = with_snapshot(json!({ "entity": decomposed }));
    assert_eq!(snapshot_entities(&item), vec![decomposed.to_owned()]);
}

/// An entity that carries no name after trimming is empty, not
/// `[""]`.
#[test]
fn empty_and_whitespace_only_entities_are_empty() {
    for raw in ["", "   ", "\t\n "] {
        let item = with_snapshot(json!({ "entity": raw }));
        assert_eq!(snapshot_entities(&item), Vec::<String>::new(), "{raw:?}");
    }
}

/// a non-string `entity` is not a name, so it is never exposed.
#[test]
fn non_string_entity_values_are_empty() {
    for raw in [
        json!(null),
        json!(7),
        json!(1.5),
        json!(true),
        json!(["dark mode"]),
        json!({"name": "dark mode"}),
    ] {
        let item = with_snapshot(json!({ "entity": raw }));
        assert_eq!(snapshot_entities(&item), Vec::<String>::new(), "{raw}");
    }
}

/// an item with no snapshot is empty and does not fail.
#[test]
fn snapshot_less_item_is_empty() {
    let mut item = fx::item("itm-n", "bank-e", "no snapshot");
    item.snapshot = None;
    assert_eq!(snapshot_entities(&item), Vec::<String>::new());
}

/// a snapshot that is not a JSON object has no `entity` field.
#[test]
fn non_object_snapshots_are_empty() {
    for raw in [json!(["entity"]), json!("entity"), json!(3), json!(null)] {
        let item = with_snapshot(raw.clone());
        assert_eq!(snapshot_entities(&item), Vec::<String>::new(), "{raw}");
    }
}

/// the triple carrier snapshot is `{subject, predicate, object}`.
#[test]
fn triple_carrier_is_empty() {
    let item = with_snapshot(json!({
        "subject": "rust",
        "predicate": "used_by",
        "object": "agent",
    }));
    assert_eq!(snapshot_entities(&item), Vec::<String>::new());
}

/// the hub-distill snapshot carries cluster ids, not an entity.
#[test]
fn hub_distilled_item_is_empty() {
    let item = with_snapshot(json!({
        "hub_id": "itm-hub",
        "neighbor_ids": ["itm-a", "itm-b"],
        "neighbor_count": 2,
    }));
    assert_eq!(snapshot_entities(&item), Vec::<String>::new());
}

/// a belief object is not a snapshot-bearing item, and the belief
/// item shape written by the store has no snapshot at all.
#[test]
fn belief_item_is_empty() {
    let mut item = fx::belief_item("itm-b", "bank-e", "belief");
    item.snapshot = None;
    assert_eq!(snapshot_entities(&item), Vec::<String>::new());
}

/// a non-default extractive field list stores other fields only.
#[test]
fn non_default_field_list_is_empty() {
    let item = with_snapshot(json!({"metric": "p95", "service": "api"}));
    assert_eq!(snapshot_entities(&item), Vec::<String>::new());
}

/// The documented empty-coverage cases, as one table. Every row must stay empty
/// so a future edit cannot silently start inventing an entity name.
#[test]
fn every_documented_empty_coverage_shape_stays_empty() {
    let mut shapes: Vec<(&str, Option<Value>)> = vec![
        (
            "triple carrier",
            Some(json!({"subject": "a", "predicate": "b", "object": "c"})),
        ),
        (
            "hub distill",
            Some(json!({"hub_id": "h", "neighbor_ids": [], "neighbor_count": 0})),
        ),
        ("belief", None),
        ("snapshot-less", None),
        ("non-default fields", Some(json!({"metric": "p95"}))),
    ];
    shapes.push(("non-object snapshot", Some(json!(["entity"]))));
    for (label, snapshot) in shapes {
        let mut item = fx::item("itm-x", "bank-e", label);
        item.snapshot = snapshot;
        assert_eq!(
            snapshot_entities(&item),
            Vec::<String>::new(),
            "{label} must stay empty"
        );
    }
}

/// Only the snapshot is the source: a gist that names an entity adds nothing.
#[test]
fn the_gist_never_contributes_an_entity() {
    let mut item = fx::item("itm-g", "bank-e", "the user likes dark mode");
    item.snapshot = Some(json!({"amount": 1}));
    assert_eq!(
        snapshot_entities(&item),
        Vec::<String>::new(),
        "the prose half is non-authoritative and must not be mined for names"
    );
}
