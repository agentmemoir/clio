// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the triples half of the live-Postgres
//! scope-ledger tests: one fixture triple per subject-link class, and the
//! assertions that the triples family row links through the carrier item's
//! subject, never through the SPO subject.
//!
//! ## Owns
//! - The Postgres fixture triples for the selected, other, shared, and
//!   unknown classes, written through the store's own public write path
//! - The live-Postgres assertions about the triples row and what each
//!   class's triple reads back as
//!
//! ## Does not own
//! - The ledger and its row plan (`inventory.rs`, `inventory_rows.rs`)
//! - The Postgres harness and the other link-class writes
//!   (`inventory_pg_support_tests.rs`)
//!
//! ## Boundary
//!
//! Changes to what the fixture triples demonstrate, or to how the triples
//! row is asserted, should normally require changes here. Changes to the
//! row plan or the harness should not.
//!
//! Keep this module focused on the triples link class.

use super::{pg_fixture_or_skip, pg_ledger, row_for};
use crate::inventory_fixture::{OTHER, SELECTED, SHARED, UNKNOWN, fixture_id};
use crate::inventory_pg_support_tests::{PgLedgerFixture, carrier};
use clio_store::{PostgresStore, Store};
use clio_types::{EpisodicType, EpistemicKind, MemoryItem, TripleRecord};

/// The actor label the fixture triples are stamped with.
const ACTOR: &str = "harness";
/// Timestamp every fixture triple is written with.
const TS: &str = "2026-06-01T00:00:00Z";

/// One fixture triple, named and carried by the same item id the SQLite
/// fixture uses, so both backends assert the same names.
fn triple(
    bank: &str,
    id: &str,
    carrier_subject: &str,
    spo: &str,
    predicate: &str,
    object: &str,
) -> (MemoryItem, TripleRecord) {
    let carrier_item = carrier(id, bank, carrier_subject, EpisodicType::Triple);
    let edge = TripleRecord {
        context: None,
        id: id.into(),
        bank_id: bank.into(),
        item_id: carrier_item.id.clone(),
        subject: spo.into(),
        predicate: predicate.into(),
        object: object.into(),
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.8),
        valid_from: TS.into(),
        valid_until: None,
        tx_from: TS.into(),
        tx_until: None,
        snapshot_ref: None,
        gist_ref: None,
        created_at: TS.into(),
        updated_at: None,
    };
    (carrier_item, edge)
}

/// Write the fixture triples through the store's own public write path, one
/// per subject-link class: the selected and other classes carried by their
/// own subject's item, the shared class carried by the selected subject's
/// item, and the unknown class carried by the erased subject's own item so
/// the erasure purges it.
pub(crate) fn write_triple_class(store: &PostgresStore, bank: &str, erased_bank: &str) {
    for subject in [SELECTED, OTHER] {
        let id = fixture_id(subject, "triple");
        let (carrier_item, edge) = triple(
            bank,
            &id,
            subject,
            "attribute",
            &format!("attribute-{subject}"),
            &format!("the {subject} attribute value"),
        );
        store
            .commit_triple_add(&carrier_item, &edge, false, ACTOR)
            .expect("write the fixture triple");
    }
    let (shared_carrier, shared_edge) = triple(
        bank,
        &fixture_id(SHARED, "triple"),
        SELECTED,
        SHARED,
        "attribute-shared",
        "the shared attribute value",
    );
    store
        .commit_triple_add(&shared_carrier, &shared_edge, false, ACTOR)
        .expect("write the fixture shared triple");
    let (unknown_carrier, unknown_edge) = triple(
        erased_bank,
        &fixture_id(UNKNOWN, "triple"),
        UNKNOWN,
        "attribute",
        "attribute-erased",
        "the erased subject's attribute value",
    );
    store
        .commit_triple_add(&unknown_carrier, &unknown_edge, false, ACTOR)
        .expect("write the erased subject's triple");
}

/// The bank a class's triple lives in: the unknown class in the erased bank,
/// so its purged-by-the-erasure claim is measured, not assumed.
fn triple_bank<'a>(class: &str, fixture: &'a PgLedgerFixture) -> &'a str {
    if class == UNKNOWN {
        &fixture.erased_bank
    } else {
        &fixture.bank
    }
}

/// Every class's triple reads back from the live store under its carrier
/// item's subject link, and the erased subject's triple is purged.
#[test]
fn inventory_pg_triples_link_through_the_carrier_items_subject() {
    let Some((_guard, fixture)) = pg_fixture_or_skip() else {
        return;
    };
    let store = &fixture.store;
    let ledger = pg_ledger(&fixture);

    let row = row_for(&ledger, "triples");
    assert_eq!(
        row.subject_link.as_deref(),
        Some("triples.item_id -> items.subject_id"),
        "the triples row names the carrier-item link"
    );
    assert_eq!(
        row.gap.as_deref(),
        Some("no data-subject column: the SPO subject is not a data-subject id"),
        "the triples row names the SPO-subject gap"
    );

    // selected and other: the triple's only subject link runs through its
    // carrier item's subject; the SPO subject names no data subject.
    for subject in [SELECTED, OTHER] {
        let edge = store
            .get_triple(&fixture.bank, &fixture_id(subject, "triple"))
            .unwrap()
            .expect("the live fixture triple");
        assert_eq!(edge.subject, "attribute");
        let carrier_item = store
            .get_memory_item(&fixture.bank, &edge.item_id)
            .unwrap()
            .expect("the triple's carrier item");
        assert_eq!(carrier_item.subject_id, subject);
    }

    // shared: the SPO subject is the shared class marker, never a
    // data-subject id, and the carrier item belongs to the selected subject.
    let shared = store
        .get_triple(&fixture.bank, &fixture_id(SHARED, "triple"))
        .unwrap()
        .expect("the live shared fixture triple");
    assert_eq!(shared.subject, SHARED);
    let shared_carrier = store
        .get_memory_item(&fixture.bank, &shared.item_id)
        .unwrap()
        .expect("the shared triple's carrier item");
    assert_eq!(shared_carrier.subject_id, SELECTED);

    // unknown: the triple names the erased subject through its carrier item,
    // so the erasure purges it; the tombstone is the row that survives.
    assert_eq!(triple_bank(UNKNOWN, &fixture), fixture.erased_bank.as_str());
    let purged = store
        .get_triple(&fixture.erased_bank, &fixture_id(UNKNOWN, "triple"))
        .unwrap();
    assert!(
        purged.is_none(),
        "the erasure purges the erased subject's triples: {purged:?}"
    );
}
