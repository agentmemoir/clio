// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the timestamp a reconciliation revision
//! records on the unit it re-derived: the record's `distilled_at` and the
//! revision item's own time columns are one canonical value, taken from the
//! engine's injected clock rather than from the wall clock.
//!
//! ## Owns
//!
//! Test-only. Never compiled into production builds.
//!
//! - The canonical revision stamp a reconcile writes, at the pinned engine clock
//! - The recorded stamp being the revision item's own creation time
//!
//! ## Does not own
//! - The reconcile scenarios themselves, and the shared fixture
//!   (`hub_distill_reconcile_tests`)
//! - The canonical form's conversion (`clio-store`'s migrate helpers)
//! - The wave envelope (`hub_distill`) or the claim lifecycle (`hub_distill_claim`)
//!
//! ## Boundary
//!
//! Changes to which moment a revision is stamped with belong here. Changes to
//! what a revision contains do not.
//!
//! Keep this module focused on the revision stamp.

use crate::hub_distill::hub_distill_reconcile_tests::{
    BANK, HUB, UNIT, add_source, first_wave, record_of, seeded, wave,
};
use clio_store::Store;

/// The canonical shape a revision stamp must have: UTC, second precision, no
/// fraction and no offset. The stamp comes from the injected clock, so the test
/// asserts the form and the agreement between the record and the item, never a
/// value read from the wall clock.
const CANONICAL_STAMP_LEN: usize = 20;

#[test]
fn a_revision_stamps_the_unit_in_canonical_seconds() {
    let (store, mut engine) = seeded();
    first_wave(&mut engine);
    add_source(&store, "n4");

    let report = wave(&engine);
    assert_eq!(report.revised, vec![HUB.to_owned()], "{report:?}");

    let record = record_of(&store, UNIT);
    assert_eq!(record.revision, 2, "the wave wrote the next revision");
    let stamp = record.distilled_at.as_str();
    assert_eq!(
        stamp.len(),
        CANONICAL_STAMP_LEN,
        "a revision stamp is `YYYY-MM-DDThh:mm:ssZ`, got {stamp:?}"
    );
    assert!(
        stamp.ends_with('Z') && stamp.as_bytes()[10] == b'T',
        "a revision stamp is UTC second precision, got {stamp:?}"
    );
    assert!(
        !stamp.bytes().all(|b| b.is_ascii_digit()),
        "an epoch-millis digit string is not a canonical stamp, got {stamp:?}"
    );

    // The revision item keeps the timeline of the cluster it summarizes: it
    // inherits the hub's creation time, so a unit is not "created" later than
    // the evidence behind it.
    let current = store
        .get_memory_item(BANK, &record.current_revision_item_id())
        .unwrap()
        .unwrap();
    let hub = store.get_memory_item(BANK, HUB).unwrap().unwrap();
    assert_eq!(
        current.created_at, hub.created_at,
        "a revision inherits its hub's creation time"
    );
}
