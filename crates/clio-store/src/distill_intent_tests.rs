// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the SQLite distill-intent store contract
//! tests: the claim write, the completion write that never downgrades, bank
//! scope, validation refusals, and the row's own shape.
//!
//! ## Owns
//! - SQLite intent claim/complete/read scenarios and the shared `claim` fixture
//!
//! ## Does not own
//! - The intent shape (`distill_intent`) or its SQL
//!   (`sqlite_distill_intent`)
//! - The Postgres intent scenarios, which need a live server
//!
//! ## Boundary
//!
//! Changes to the SQLite intent store contract should normally require changes
//! here. Changes to the intent shape should not require changes here.
//!
//! Keep this module focused on SQLite intent storage scenarios.
//!
//! Test-only. Never compiled into production builds.

use crate::distill_intent::DistillIntent;
use crate::distilled_unit_store::DistilledUnitStore;
use crate::erase_test_support::{local_kms, unique};
use crate::migrate::IntoAmError;
use crate::sqlite::SqliteStore;
use clio_types::ErrorCode;
use std::collections::BTreeSet;
use std::sync::Arc;

/// A fixed claim stamp in the format a derivation record's `distilled_at` uses:
/// millisecond-epoch text. A literal, never the system clock, so nothing here
/// depends on when the suite ran.
const CLAIMED_AT: &str = "1780272000000";

/// A well-formed claim: a bank, a hub, the deterministic unit id for that hub,
/// and the wave's actor.
fn claim(bank: &str, hub: &str) -> DistillIntent {
    DistillIntent::new(
        bank,
        hub,
        format!("hub-distill:{bank}:{hub}"),
        "tester",
        CLAIMED_AT,
    )
}

#[test]
fn sqlite_a_claim_is_read_back_with_the_fields_it_was_written_with() {
    let store = SqliteStore::open_in_memory(local_kms()).unwrap();
    let bank = unique("sqlite-intent-claim");
    let hub = unique("hub");
    let intent = claim(&bank, &hub);

    assert!(store.write_distill_intent(&bank, &intent).unwrap());

    let read = store.get_distill_intent(&bank, &hub).unwrap().unwrap();
    assert_eq!(read, intent);
    assert_eq!(read.bank_id, bank);
    assert_eq!(read.hub_id, hub);
    assert_eq!(read.actor, "tester");
    assert_eq!(read.claimed_at, CLAIMED_AT);
    assert_eq!(read.revision_item_id, "");
    assert!(!read.is_complete(), "a fresh claim is not complete");
}

#[test]
fn sqlite_a_second_claim_for_the_same_hub_changes_nothing() {
    let store = SqliteStore::open_in_memory(local_kms()).unwrap();
    let bank = unique("sqlite-intent-second-claim");
    let hub = unique("hub");
    let first = claim(&bank, &hub);
    assert!(store.write_distill_intent(&bank, &first).unwrap());
    let revision = unique("revision");
    assert!(
        store
            .complete_distill_intent(&bank, &hub, &revision)
            .unwrap()
    );

    // A later wave claiming the same hub is refused, and the finished record
    // keeps the first wave's actor, unit id, stamp, and revision.
    let second = DistillIntent::new(
        &bank,
        &hub,
        unique("unit"),
        "another-actor",
        "1783000000000",
    );
    assert!(!store.write_distill_intent(&bank, &second).unwrap());

    let mut expected = first;
    expected.revision_item_id = revision.clone();
    let read = store.get_distill_intent(&bank, &hub).unwrap().unwrap();
    assert_eq!(read, expected);
    assert_eq!(read.actor, "tester");
    assert_eq!(read.claimed_at, CLAIMED_AT);
    assert_eq!(read.revision_item_id, revision);
}

#[test]
fn sqlite_completing_a_claim_records_the_produced_revision() {
    let store = SqliteStore::open_in_memory(local_kms()).unwrap();
    let bank = unique("sqlite-intent-complete");
    let hub = unique("hub");
    let intent = claim(&bank, &hub);
    assert!(store.write_distill_intent(&bank, &intent).unwrap());
    let revision = unique("revision");

    assert!(
        store
            .complete_distill_intent(&bank, &hub, &revision)
            .unwrap()
    );

    let read = store.get_distill_intent(&bank, &hub).unwrap().unwrap();
    assert!(read.is_complete());
    assert_eq!(read.revision_item_id, revision);
    assert_eq!(read.actor, "tester");
    assert_eq!(read.claimed_at, CLAIMED_AT);
}

#[test]
fn sqlite_a_second_completion_keeps_the_first_revision() {
    let store = SqliteStore::open_in_memory(local_kms()).unwrap();
    let bank = unique("sqlite-intent-no-downgrade");
    let hub = unique("hub");
    assert!(
        store
            .write_distill_intent(&bank, &claim(&bank, &hub))
            .unwrap()
    );
    let first = unique("revision");
    assert!(store.complete_distill_intent(&bank, &hub, &first).unwrap());

    // A retried or duplicated completion names a different revision and is
    // refused: the claim is never downgraded to a weaker state.
    assert!(
        !store
            .complete_distill_intent(&bank, &hub, &unique("revision"))
            .unwrap()
    );

    let read = store.get_distill_intent(&bank, &hub).unwrap().unwrap();
    assert_eq!(read.revision_item_id, first);
}

#[test]
fn sqlite_completing_an_unclaimed_hub_reports_nothing_completed() {
    let store = SqliteStore::open_in_memory(local_kms()).unwrap();
    let bank = unique("sqlite-intent-unclaimed-complete");

    assert!(
        !store
            .complete_distill_intent(&bank, &unique("hub"), &unique("revision"))
            .unwrap()
    );
}

#[test]
fn sqlite_an_unclaimed_hub_reads_as_none() {
    let store = SqliteStore::open_in_memory(local_kms()).unwrap();
    let bank = unique("sqlite-intent-missing");

    assert!(
        store
            .get_distill_intent(&bank, &unique("hub"))
            .unwrap()
            .is_none()
    );
}

#[test]
fn sqlite_another_bank_cannot_read_a_claim() {
    let store = SqliteStore::open_in_memory(local_kms()).unwrap();
    let bank = unique("sqlite-intent-scope");
    let hub = unique("hub");
    assert!(
        store
            .write_distill_intent(&bank, &claim(&bank, &hub))
            .unwrap()
    );

    assert!(
        store
            .get_distill_intent(&unique("other-bank"), &hub)
            .unwrap()
            .is_none(),
        "another bank must not resolve the claim"
    );
}

#[test]
fn sqlite_another_bank_cannot_complete_a_claim() {
    let store = SqliteStore::open_in_memory(local_kms()).unwrap();
    let bank = unique("sqlite-intent-complete-scope");
    let hub = unique("hub");
    let intent = claim(&bank, &hub);
    assert!(store.write_distill_intent(&bank, &intent).unwrap());

    let other = unique("sqlite-intent-complete-other");
    let revision = unique("revision");
    let finished = store
        .complete_distill_intent(&other, &hub, &revision)
        .unwrap();
    assert!(!finished, "another bank finishes nothing");
}

#[test]
fn sqlite_concurrent_completions_have_exactly_one_winner() {
    let store = Arc::new(SqliteStore::open_in_memory(local_kms()).unwrap());
    let bank = unique("sqlite-intent-race");
    let hub = unique("hub");
    let intent = claim(&bank, &hub);
    assert!(store.write_distill_intent(&bank, &intent).unwrap());

    let won: Vec<bool> = (0..2)
        .map(|_| {
            let s = Arc::clone(&store);
            let (bank, hub) = (bank.clone(), hub.clone());
            let revision = unique("revision");
            std::thread::spawn(move || s.complete_distill_intent(&bank, &hub, &revision).unwrap())
        })
        .map(|h| h.join().unwrap())
        .collect();
    assert_eq!(won.iter().filter(|w| **w).count(), 1, "one winner: {won:?}");
}

#[test]
fn sqlite_another_banks_claim_for_the_same_hub_leaves_the_first_alone() {
    let store = SqliteStore::open_in_memory(local_kms()).unwrap();
    let bank = unique("sqlite-intent-isolation");
    let other = unique("sqlite-intent-isolation-other");
    let hub = unique("hub");
    let first = claim(&bank, &hub);
    assert!(store.write_distill_intent(&bank, &first).unwrap());

    assert!(
        store
            .write_distill_intent(&other, &claim(&other, &hub))
            .unwrap(),
        "the same hub id in another bank is a different claim"
    );

    assert_eq!(
        store.get_distill_intent(&bank, &hub).unwrap().unwrap(),
        first
    );
    assert_eq!(
        store
            .get_distill_intent(&other, &hub)
            .unwrap()
            .unwrap()
            .bank_id,
        other
    );
}

#[test]
fn sqlite_an_invalid_claim_is_refused_and_stored_nowhere() {
    let store = SqliteStore::open_in_memory(local_kms()).unwrap();
    let bank = unique("sqlite-intent-invalid");
    let hub = unique("hub");

    // An empty bank is a missing bank, the code every other store writer uses.
    let mut no_bank = claim(&bank, &hub);
    no_bank.bank_id = String::new();
    assert_eq!(
        store
            .write_distill_intent(&bank, &no_bank)
            .unwrap_err()
            .code,
        ErrorCode::MissingBank
    );

    // Every other field is required: a claim missing one of them is not a
    // statement about any hub.
    let blanks: [fn(&mut DistillIntent); 4] = [
        |r| r.hub_id = String::new(),
        |r| r.unit_item_id = String::new(),
        |r| r.actor = String::new(),
        |r| r.claimed_at = String::new(),
    ];
    for blank in blanks {
        let mut bad = claim(&bank, &hub);
        blank(&mut bad);
        assert_eq!(
            store.write_distill_intent(&bank, &bad).unwrap_err().code,
            ErrorCode::InvalidArgument,
            "{bad:?}"
        );
    }

    assert!(
        store.get_distill_intent(&bank, &hub).unwrap().is_none(),
        "a refused claim must leave no row behind"
    );
}

#[test]
fn sqlite_a_claim_handed_a_revision_up_front_is_refused() {
    let store = SqliteStore::open_in_memory(local_kms()).unwrap();
    let bank = unique("sqlite-intent-preset");
    let hub = unique("hub");
    let mut done = claim(&bank, &hub);
    done.revision_item_id = unique("revision");
    let refused = store.write_distill_intent(&bank, &done);
    assert_eq!(refused.unwrap_err().code, ErrorCode::InvalidArgument);
}

#[test]
fn sqlite_a_claim_written_under_another_bank_is_refused() {
    let store = SqliteStore::open_in_memory(local_kms()).unwrap();
    let bank = unique("sqlite-intent-mismatch");
    let other = unique("sqlite-intent-mismatch-other");
    let hub = unique("hub");

    assert_eq!(
        store
            .write_distill_intent(&other, &claim(&bank, &hub))
            .unwrap_err()
            .code,
        ErrorCode::InvalidArgument
    );

    assert!(
        store.get_distill_intent(&bank, &hub).unwrap().is_none(),
        "the claim must not land in either bank"
    );
    assert!(store.get_distill_intent(&other, &hub).unwrap().is_none());
}

#[test]
fn sqlite_a_completion_naming_nothing_to_record_is_refused() {
    let store = SqliteStore::open_in_memory(local_kms()).unwrap();
    let bank = unique("sqlite-intent-bad-complete");
    let hub = unique("hub");
    assert!(
        store
            .write_distill_intent(&bank, &claim(&bank, &hub))
            .unwrap()
    );

    assert_eq!(
        store
            .complete_distill_intent("", &hub, &unique("revision"))
            .unwrap_err()
            .code,
        ErrorCode::MissingBank
    );
    assert_eq!(
        store
            .complete_distill_intent(&bank, "", &unique("revision"))
            .unwrap_err()
            .code,
        ErrorCode::InvalidArgument
    );
    assert_eq!(
        store
            .complete_distill_intent(&bank, &hub, "")
            .unwrap_err()
            .code,
        ErrorCode::InvalidArgument
    );

    let read = store.get_distill_intent(&bank, &hub).unwrap().unwrap();
    assert!(
        !read.is_complete(),
        "a refused completion must leave the claim incomplete"
    );
}

#[test]
fn sqlite_the_claim_row_carries_only_the_six_claim_columns() {
    let store = SqliteStore::open_in_memory(local_kms()).unwrap();

    let shape = store
        .with_conn(|conn| {
            let mut stmt = conn
                .prepare("PRAGMA table_info(distill_intents)")
                .am("intent shape prepare")?;
            let rows = stmt
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, i64>(5)?,
                    ))
                })
                .am("intent shape query")?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row.am("intent shape row")?);
            }
            Ok(out)
        })
        .unwrap();

    // Sorted before comparing: the read order of a pragma is not the contract,
    // the exact set of columns is.
    let mut names: Vec<&str> = shape.iter().map(|(name, _, _, _)| name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        vec![
            "actor",
            "bank_id",
            "claimed_at",
            "hub_id",
            "revision_item_id",
            "unit_item_id",
        ],
        "a claim row holds ids and stamps only: a new content column must fail here"
    );
    // SQLite reports a declared type in upper case, so the type name is
    // compared case-insensitively rather than as the pragma renders it.
    let types: BTreeSet<String> = shape
        .iter()
        .map(|(_, ty, _, _)| ty.to_ascii_lowercase())
        .collect();
    assert_eq!(
        types,
        BTreeSet::from(["text".to_owned()]),
        "every column is text, so no value is coerced"
    );
    // No column is nullable: an intent has no absent state, so a partial
    // claim cannot hide behind a NULL.
    assert!(
        shape.iter().all(|(_, _, notnull, _)| *notnull == 1),
        "every column must be NOT NULL: {shape:?}"
    );
    // The key is the bank first, so one bank can never claim another's hub.
    let mut key: Vec<(i64, &str)> = shape
        .iter()
        .filter(|(_, _, _, pk)| *pk > 0)
        .map(|(name, _, _, pk)| (*pk, name.as_str()))
        .collect();
    key.sort_unstable();
    let key: Vec<&str> = key.into_iter().map(|(_, name)| name).collect();
    assert_eq!(key, vec!["bank_id", "hub_id"]);
}
