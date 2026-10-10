// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the rules a distilled unit's revision write
//! must satisfy: the item id derivation, what a revision item has to carry, the
//! prior-revision lookup, and the projection a reader walks back.
//!
//! ## Owns
//! - The revision-write rules as unit tests, including every rejection arm
//! - The record-to-snapshot projection and its reader
//!
//! ## Does not own
//! - The revision write itself against a real store
//!   (`distilled_unit_reconcile_tests`, `postgres_distilled_unit_tests`)
//! - The record shape and its evidence trail (`distilled_unit`,
//!   `distilled_evidence`)
//!
//! ## Boundary
//!
//! Changes to revision identity or to the item projection should normally
//! require changes here. Changes to record shape should not require changes
//! here.
//!
//! Keep this module focused on revision identity and the snapshot projection.
//!
//! Test-only. Never compiled into production builds.

use crate::distilled_revision::{
    projected_unit_id, revision_item_id, revision_prior_id, snapshot_projection, validate_revision,
};
use crate::distilled_unit::DistilledUnitRecord;
use crate::distilled_unit::distilled_unit_reconcile_tests::record;
use crate::distilled_unit::distilled_unit_tests::hub_item;
use clio_types::MemoryItem;

/// A revision-1 unit record over one source, matching what `hub_item` projects.
fn unit() -> DistilledUnitRecord {
    record("b", "hub-distill:b:h", "h", &["s1".to_owned()])
}

/// The item a revision write of `unit` has to carry: the record's current
/// revision id, the record's marker, and a snapshot naming its hub.
fn revision_item_of(unit: &DistilledUnitRecord) -> MemoryItem {
    hub_item(
        &unit.bank_id,
        &unit.current_revision_item_id(),
        "subj",
        &unit.hub_id,
        &["s1"],
    )
}

#[test]
fn revision_one_keeps_the_unit_id_and_later_revisions_suffix_it() {
    assert_eq!(revision_item_id("hub-distill:b:h", 0), "hub-distill:b:h");
    assert_eq!(revision_item_id("hub-distill:b:h", 1), "hub-distill:b:h");
    assert_eq!(revision_item_id("hub-distill:b:h", 2), "hub-distill:b:h:v2");
    assert_eq!(
        revision_item_id("hub-distill:b:h", 12),
        "hub-distill:b:h:v12"
    );
}

#[test]
fn a_revision_item_carrying_the_records_bank_marker_and_hub_is_accepted() {
    let unit = unit();
    validate_revision(&unit, &revision_item_of(&unit)).expect("a matching revision item");
}

#[test]
fn a_revision_item_written_into_another_bank_is_refused() {
    let unit = unit();
    let mut item = revision_item_of(&unit);
    item.bank_id = "other-bank".to_owned();
    assert!(
        validate_revision(&unit, &item).is_err(),
        "a revision cannot move a unit to another bank"
    );
}

#[test]
fn a_revision_item_under_another_id_is_refused_and_names_the_right_one() {
    let unit = unit();
    let mut item = revision_item_of(&unit);
    item.id = "hub-distill:b:h:v9".to_owned();
    let err = validate_revision(&unit, &item).expect_err("the id must match the record");
    assert!(
        err.to_string().contains("hub-distill:b:h"),
        "the error names the id the record expects: {err}"
    );
}

#[test]
fn a_revision_item_without_the_records_marker_is_refused() {
    let unit = unit();
    let mut item = revision_item_of(&unit);
    item.source_ref = Some("hand_written".to_owned());
    assert!(
        validate_revision(&unit, &item).is_err(),
        "a distilled revision must carry the distilled marker"
    );
}

#[test]
fn a_revision_whose_snapshot_names_another_hub_is_refused() {
    let unit = unit();
    let mut item = revision_item_of(&unit);
    item.snapshot = Some(serde_json::json!({"hub_id": "another-hub"}));
    assert!(
        validate_revision(&unit, &item).is_err(),
        "the snapshot must name the record's own hub"
    );
}

#[test]
fn a_revision_whose_snapshot_names_no_hub_is_refused() {
    let unit = unit();
    for snapshot in [
        serde_json::json!({}),
        serde_json::json!({"hub_id": 7}),
        serde_json::json!(["hub-distill:b:h"]),
    ] {
        let mut item = revision_item_of(&unit);
        item.snapshot = Some(snapshot.clone());
        assert!(
            validate_revision(&unit, &item).is_err(),
            "a snapshot with no hub id is refused: {snapshot}"
        );
    }
}

#[test]
fn a_revision_write_needs_a_closed_prior_revision_to_point_at() {
    assert!(
        revision_prior_id(&unit()).is_err(),
        "revision 1 has no prior to close"
    );
    let second = DistilledUnitRecord {
        revision: 2,
        ..unit()
    };
    assert_eq!(
        revision_prior_id(&second).expect("revision 2 has a prior"),
        "hub-distill:b:h",
        "revision 1 kept the unit id, so that is the row to close"
    );
}

#[test]
fn the_projection_names_the_record_a_revision_item_reader_can_find() {
    let unit = unit();
    let projection = snapshot_projection(&unit);
    assert_eq!(
        projected_unit_id(Some(&projection)),
        Some(unit.item_id.as_str()),
        "a reader holding the revision item id finds the record"
    );
}

#[test]
fn a_projection_without_a_unit_id_leaves_the_reader_nothing_to_find() {
    for snapshot in [
        None,
        Some(serde_json::json!({})),
        Some(serde_json::json!({"unit_id": ""})),
        Some(serde_json::json!({"unit_id": 7})),
        Some(serde_json::json!(["unit"])),
    ] {
        assert_eq!(
            projected_unit_id(snapshot.as_ref()),
            None,
            "no unit id, no record: {snapshot:?}"
        );
    }
}
