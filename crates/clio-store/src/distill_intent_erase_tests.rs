// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the erasure retention of a wave's distill
//! claim: a claim names two handles on the erased item — the hub it distils and
//! the unit it writes — so deleting either one must take the row with it, and
//! deleting anything else must leave it alone.
//!
//! ## Owns
//! - The claim row's fate under an item delete, a rolled-back item delete, and
//!   a subject erasure, on SQLite and on Postgres
//!
//! ## Does not own
//! - The claim, completion, and read contract (`distill_intent_tests` and the
//!   parity suite), which both backends already share
//! - The erasure transaction behind `delete_item_impl`
//!
//! ## Boundary
//!
//! Changes to which erase removes a claim should normally require changes here.
//! Changes to the claim contract should not require changes here.
//!
//! Keep this module focused on erasure retention of a distill claim.
//!
//! Test-only. Never compiled into production builds.

use crate::compliance_store::ComplianceStore;
use crate::distill_intent::DistillIntent;
use crate::distilled_unit_store::DistilledUnitStore;
use crate::erase_test_support::{local_kms, memory_item, spec, unique};
use crate::postgres::PostgresStore;
use crate::sqlite::SqliteStore;
use crate::store::Store;
use clio_types::ErrorCode;

/// The claim stamp in the millisecond-epoch text a derivation record's
/// `distilled_at` uses. A literal, never a clock read, so nothing here depends
/// on when the suite ran.
const CLAIMED_AT: &str = "1780272000000";

/// A hub item, the unit item a wave writes for it, and the claim naming both.
/// Ids are globally unique, so a second bank seeds an item under an id of its
/// own rather than reusing one this bank already holds.
fn seed_claim(store: &dyn Store, bank: &str, subject: &str) -> (String, String, DistillIntent) {
    let hub = unique("intent-hub");
    let unit = format!("hub-distill:{bank}:{hub}");
    let intent = DistillIntent::new(bank, &hub, &unit, "tester", CLAIMED_AT);
    store
        .create_memory_item(&memory_item(bank, &hub, subject), "tester")
        .unwrap();
    store
        .create_memory_item(&memory_item(bank, &unit, subject), "tester")
        .unwrap();
    assert!(store.write_distill_intent(bank, &intent).unwrap());
    (hub, unit, intent)
}

/// A Postgres store for one test, with the lock that serializes the shared
/// Compose database held for the caller's whole test body.
fn postgres_store() -> (PostgresStore, crate::PgTestGuard) {
    let guard = crate::lock_pg_test();
    let url = crate::db_url::postgres_url_from_env();
    let store = PostgresStore::open(url.as_str(), local_kms()).unwrap();
    (store, guard)
}

/// Erasing the hub takes the claim: a hub id is an item id, so it goes too.
fn suite_erase_of_a_hub_takes_the_claim_that_named_it(store: &dyn Store) {
    let bank = unique("intent-erase-hub");
    let (hub, _, _) = seed_claim(store, &bank, "s");

    assert!(store.delete_item(&bank, &hub).unwrap());

    assert!(
        store.get_distill_intent(&bank, &hub).unwrap().is_none(),
        "the claim naming a gone hub goes too"
    );
}

/// Erasing the unit takes the claim too: the unit id is the other handle.
fn suite_erase_of_the_unit_takes_the_claim_that_named_it(store: &dyn Store) {
    let bank = unique("intent-erase-unit");
    let (hub, unit, _) = seed_claim(store, &bank, "s");

    assert!(store.delete_item(&bank, &unit).unwrap());

    assert!(
        store.get_distill_intent(&bank, &hub).unwrap().is_none(),
        "the claim naming a gone unit goes too"
    );
}

/// An item the claim never named leaves the row: it is keyed on both handles.
fn suite_erase_of_an_unnamed_item_leaves_the_claim(store: &dyn Store) {
    let bank = unique("intent-erase-other");
    let (hub, _, intent) = seed_claim(store, &bank, "s");
    let other = unique("intent-unrelated");
    store
        .create_memory_item(&memory_item(&bank, &other, "s"), "tester")
        .unwrap();

    assert!(store.delete_item(&bank, &other).unwrap());

    assert_eq!(store.get_distill_intent(&bank, &hub).unwrap(), Some(intent));
}

/// An item in another bank is another subject: the delete is bank-scoped, so
/// the other bank's row is untouched even under a name this bank also holds.
fn suite_erase_in_another_bank_leaves_the_claim(store: &dyn Store) {
    let bank = unique("intent-erase-bank-scope");
    let (hub, _, intent) = seed_claim(store, &bank, "s");
    let other_bank = unique("intent-scope-other");
    let other = unique("intent-other-bank");
    store
        .create_memory_item(&memory_item(&other_bank, &other, "s"), "tester")
        .unwrap();

    assert!(store.delete_item(&other_bank, &other).unwrap());

    assert_eq!(store.get_distill_intent(&bank, &hub).unwrap(), Some(intent));
}

/// A second bank's claim may name the same hub id: deleting the first bank's
/// hub takes only the first bank's claim. Without the claim delete's bank
/// filter this test fails, because both rows name that hub.
fn suite_erase_takes_only_the_erased_banks_claim_on_a_shared_hub(store: &dyn Store) {
    let bank = unique("intent-erase-shared");
    let (hub, _, _) = seed_claim(store, &bank, "s");
    let other = unique("intent-erase-shared-other");
    let other_claim = DistillIntent::new(
        &other,
        &hub,
        format!("hub-distill:{other}:{hub}"),
        "tester",
        CLAIMED_AT,
    );
    assert!(store.write_distill_intent(&other, &other_claim).unwrap());

    assert!(store.delete_item(&bank, &hub).unwrap());

    assert!(
        store.get_distill_intent(&bank, &hub).unwrap().is_none(),
        "the erased bank's claim goes with its hub"
    );
    assert_eq!(
        store.get_distill_intent(&other, &hub).unwrap(),
        Some(other_claim),
        "another bank's claim on the same hub id stays"
    );
}

/// Erasing either handle takes a completed claim too: completion is not a
/// retention grant, the row goes with the hub or the unit it names.
fn suite_erase_takes_a_completed_claim(store: &dyn Store) {
    let bank = unique("intent-erase-complete");
    let (hub, _, _) = seed_claim(store, &bank, "s");
    let revision = unique("intent-erase-revision");
    assert!(
        store
            .complete_distill_intent(&bank, &hub, &revision)
            .unwrap()
    );

    assert!(store.delete_item(&bank, &hub).unwrap());

    assert!(
        store.get_distill_intent(&bank, &hub).unwrap().is_none(),
        "a completed claim goes with the erased hub"
    );
}

/// The compliance path is not the per-item delete: erasing the subject shreds
/// its items but keeps the claim row, so the claim follows the erased rows'
/// metadata retention in that path.
fn suite_subject_erasure_retains_the_claim<S: Store + ComplianceStore>(store: &S) {
    let bank = unique("intent-subject-erase");
    let subject = unique("intent-claim-subject");
    let (hub, _, intent) = seed_claim(store, &bank, &subject);

    store
        .erase_subject(&spec(&bank, &subject, &unique("req")))
        .unwrap();
    assert!(
        store.subject_dek_destroyed(&subject).unwrap(),
        "the erase itself ran"
    );
    assert_eq!(
        store.get_distill_intent(&bank, &hub).unwrap(),
        Some(intent),
        "the compliance path retains the claim row"
    );
}

/// A failure inside the delete transaction rolls the whole sequence back, so
/// the claim is still present: it is deleted in the same transaction as the
/// item, never before it.
#[test]
fn sqlite_a_rolled_back_delete_leaves_the_claim_present() {
    let store = SqliteStore::open_in_memory(local_kms()).unwrap();
    let bank = unique("intent-erase-rollback");
    let (hub, _, intent) = seed_claim(&store, &bank, "s");
    // A BEFORE DELETE trigger aborts the claim delete inside the transaction.
    store
        .test_execute(
            "CREATE TRIGGER fail_intent_delete BEFORE DELETE ON distill_intents
             BEGIN SELECT RAISE(ABORT, 'forced intent delete failure'); END",
        )
        .unwrap();

    let err = store.delete_item(&bank, &hub).unwrap_err();

    assert_eq!(err.code, ErrorCode::Internal, "{err}");
    assert_eq!(
        store.get_distill_intent(&bank, &hub).unwrap(),
        Some(intent),
        "the rolled-back delete leaves the claim present"
    );
    assert!(
        store.get_memory_item(&bank, &hub).unwrap().is_some(),
        "and the hub item it named is present too"
    );
}

/// The Postgres delete runs in one transaction too: a forced failure in the
/// claim delete rolls the whole sequence back, and the claim stays present.
#[test]
fn postgres_a_rolled_back_delete_leaves_the_claim_present() {
    let (store, _guard) = postgres_store();
    let bank = unique("intent-erase-rollback");
    let (hub, _, intent) = seed_claim(&store, &bank, "s");
    // A BEFORE DELETE trigger aborts the claim delete inside the transaction.
    // The DDL is idempotent and dropped before any assertion, because the
    // server is shared with every other Postgres test.
    store
        .test_execute(
            "CREATE OR REPLACE FUNCTION fail_intent_delete() RETURNS trigger
             LANGUAGE plpgsql AS $$
             BEGIN RAISE EXCEPTION 'forced intent delete failure'; END $$",
        )
        .unwrap();
    let drop = "DROP TRIGGER IF EXISTS fail_intent_delete ON distill_intents";
    let create = "CREATE TRIGGER fail_intent_delete BEFORE DELETE ON distill_intents
         FOR EACH ROW EXECUTE FUNCTION fail_intent_delete()";
    store.test_execute(create).unwrap();

    let deleted = store.delete_item(&bank, &hub);

    store.test_execute(drop).unwrap();
    store
        .test_execute("DROP FUNCTION IF EXISTS fail_intent_delete()")
        .unwrap();
    assert!(deleted.is_err(), "{deleted:?}");
    assert_eq!(
        store.get_distill_intent(&bank, &hub).unwrap(),
        Some(intent),
        "the rolled-back delete leaves the claim present"
    );
}

#[test]
fn sqlite_erase_of_a_hub_takes_the_claim_that_named_it() {
    let store = SqliteStore::open_in_memory(local_kms()).unwrap();
    suite_erase_of_a_hub_takes_the_claim_that_named_it(&store);
}

#[test]
fn postgres_erase_of_a_hub_takes_the_claim_that_named_it() {
    let (store, _guard) = postgres_store();
    suite_erase_of_a_hub_takes_the_claim_that_named_it(&store);
}

#[test]
fn sqlite_erase_of_the_unit_takes_the_claim_that_named_it() {
    let store = SqliteStore::open_in_memory(local_kms()).unwrap();
    suite_erase_of_the_unit_takes_the_claim_that_named_it(&store);
}

#[test]
fn postgres_erase_of_the_unit_takes_the_claim_that_named_it() {
    let (store, _guard) = postgres_store();
    suite_erase_of_the_unit_takes_the_claim_that_named_it(&store);
}

#[test]
fn sqlite_erase_of_an_unnamed_item_leaves_the_claim() {
    let store = SqliteStore::open_in_memory(local_kms()).unwrap();
    suite_erase_of_an_unnamed_item_leaves_the_claim(&store);
}

#[test]
fn postgres_erase_of_an_unnamed_item_leaves_the_claim() {
    let (store, _guard) = postgres_store();
    suite_erase_of_an_unnamed_item_leaves_the_claim(&store);
}

#[test]
fn sqlite_erase_in_another_bank_leaves_the_claim() {
    let store = SqliteStore::open_in_memory(local_kms()).unwrap();
    suite_erase_in_another_bank_leaves_the_claim(&store);
}

#[test]
fn postgres_erase_in_another_bank_leaves_the_claim() {
    let (store, _guard) = postgres_store();
    suite_erase_in_another_bank_leaves_the_claim(&store);
}

#[test]
fn sqlite_erase_takes_only_the_erased_banks_claim_on_a_shared_hub() {
    let store = SqliteStore::open_in_memory(local_kms()).unwrap();
    suite_erase_takes_only_the_erased_banks_claim_on_a_shared_hub(&store);
}

#[test]
fn postgres_erase_takes_only_the_erased_banks_claim_on_a_shared_hub() {
    let (store, _guard) = postgres_store();
    suite_erase_takes_only_the_erased_banks_claim_on_a_shared_hub(&store);
}

#[test]
fn sqlite_erase_takes_a_completed_claim() {
    let store = SqliteStore::open_in_memory(local_kms()).unwrap();
    suite_erase_takes_a_completed_claim(&store);
}

#[test]
fn postgres_erase_takes_a_completed_claim() {
    let (store, _guard) = postgres_store();
    suite_erase_takes_a_completed_claim(&store);
}

#[test]
fn sqlite_subject_erasure_retains_the_claim() {
    suite_subject_erasure_retains_the_claim(&SqliteStore::open_in_memory(local_kms()).unwrap());
}

#[test]
fn postgres_subject_erasure_retains_the_claim() {
    let (store, _guard) = postgres_store();
    suite_subject_erasure_retains_the_claim(&store);
}
