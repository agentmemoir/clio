// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! This module is responsible for the Postgres parity of the per-revision
//! evidence trail and the claim-state read: the same record round-trip, the same
//! pre-v3 empty-trail read, and the same live/discarded/absent answers the
//! SQLite backend gives, so the two cannot drift.
//!
//! ## Owns
//! - Postgres evidence round-trip, revision append, and claim-state scenarios
//!
//! ## Does not own
//! - Production SQL (`postgres_distilled_unit`, `postgres_distilled_claim`)
//! - The SQLite equivalents (`distilled_evidence_tests`,
//!   `distilled_freshness_tests`)
//!
//! ## Boundary
//!
//! Changes to how Postgres stores or reads the trail should normally require
//! changes here. Changes to the trail's shape should require
//! `distilled_evidence`.
//!
//! Test-only. Never compiled into production builds.

use crate::distilled_evidence::{EvidenceEntry, evidence_from_items};
use crate::distilled_freshness::ClaimedSourceState;
use crate::distilled_unit::distilled_unit_tests::hub_item;
use crate::distilled_unit::{DISTILLED_UNIT_RECORD_VERSION, DistilledUnitRecord};
use crate::distilled_unit_store::DistilledUnitStore;
use crate::erase_test_support::{local_kms, memory_item, unique};
use crate::migrate::IntoAmError;
use crate::postgres::PostgresStore;
use crate::store::Store;

/// A store plus the shared-Postgres lock guard, which must outlive the test.
fn pg_store() -> (crate::PgTestGuard, PostgresStore) {
    let guard = crate::lock_pg_test();
    let url = crate::db_url::postgres_url_from_env();
    let store = PostgresStore::open(url.as_str(), local_kms()).unwrap();
    (guard, store)
}

/// An item with an explicit `updated_at`, so no assertion reads a real clock.
fn stamped(bank: &str, id: &str, updated_at: &str) -> clio_types::MemoryItem {
    let mut item = memory_item(bank, id, "subj");
    item.updated_at = updated_at.to_owned();
    item
}

/// The stored trail round-trips through the Postgres column, and a record
/// written before the column existed reads as an empty trail rather than
/// failing: the same backward-compatible read the SQLite backend gives.
#[test]
fn postgres_the_trail_round_trips_and_a_pre_v3_row_reads_empty() {
    let (_guard, store) = pg_store();
    let bank = unique("pg-evidence");
    let subject = unique("subject");
    let hub = unique("hub");
    let s1 = unique("s1");
    let item_id = format!("hub-distill:{bank}:{hub}");
    let item = hub_item(&bank, &item_id, &subject, &hub, &[&s1]);
    let mut r = DistilledUnitRecord::new(
        &bank,
        &item_id,
        &hub,
        [s1.clone()],
        "hub_distilled",
        "1767225600000",
    );
    r.record_evidence(
        evidence_from_items([&stamped(&bank, &s1, "1767139200000")]),
        "1767225600000",
    );
    assert_eq!(r.record_version, DISTILLED_UNIT_RECORD_VERSION);
    store.commit_distilled_unit(&item, &r, "tester").unwrap();

    let read = store
        .get_distilled_unit(&bank, &item_id)
        .unwrap()
        .expect("record");
    assert_eq!(read.evidence, r.evidence, "the trail round-trips");
    assert_eq!(read.proof_count(), 1);
    assert_eq!(
        read.current_evidence()[0].as_of.as_deref(),
        Some("1767139200000")
    );

    store
        .with_client(|client| {
            client
                .execute(
                    "UPDATE distilled_units SET evidence_json = '[]' WHERE bank_id = $1",
                    &[&bank],
                )
                .am("clear the stored trail pg")
        })
        .unwrap();
    let pre_v3 = store
        .get_distilled_unit(&bank, &item_id)
        .unwrap()
        .expect("record");
    assert!(pre_v3.evidence.is_empty(), "a pre-v3 row reads empty");
    assert_eq!(pre_v3.proof_count(), 0, "and claims no proof");
}

/// A revision appends a list and keeps the earlier one, exactly as SQLite does.
#[test]
fn postgres_a_revision_appends_and_keeps_the_earlier_list() {
    let (_guard, store) = pg_store();
    let bank = unique("pg-evidence-revision");
    let subject = unique("subject");
    let hub = unique("hub");
    let s1 = unique("s1");
    let s2 = unique("s2");
    let item_id = format!("hub-distill:{bank}:{hub}");
    let item = hub_item(&bank, &item_id, &subject, &hub, &[&s1]);
    let mut r = DistilledUnitRecord::new(
        &bank,
        &item_id,
        &hub,
        [s1.clone()],
        "hub_distilled",
        "1767225600000",
    );
    r.record_evidence(
        evidence_from_items([&stamped(&bank, &s1, "1767139200000")]),
        "1767225600000",
    );
    store.commit_distilled_unit(&item, &r, "tester").unwrap();
    let first = store
        .get_distilled_unit(&bank, &item_id)
        .unwrap()
        .unwrap()
        .evidence;

    let mut second = r.clone();
    second.revision = 2;
    second.distilled_at = "1767312000000".to_owned();
    second.record_evidence(
        vec![
            EvidenceEntry::new(s1.clone(), Some("1767225600000")).unwrap(),
            EvidenceEntry::new(s2.clone(), Some("1767225600000")).unwrap(),
        ],
        "1767312000000",
    );
    let rev2 = second.current_revision_item_id();
    let mut rev2_item = hub_item(&bank, &rev2, &subject, &hub, &[&s1, &s2]);
    rev2_item.snapshot = Some(crate::snapshot_projection(&second));
    store
        .revise_distilled_unit(&second, &rev2_item, "tester")
        .unwrap();

    let read = store
        .get_distilled_unit(&bank, &item_id)
        .unwrap()
        .expect("record");
    assert_eq!(read.evidence[0], first[0], "revision 1 is untouched");
    assert_eq!(read.evidence_history().len(), 2, "two revisions");
    assert_eq!(read.proof_count(), 2, "the count follows the current one");
}

/// The claim-state read gives the same three answers on Postgres: a live source
/// reports its stamp, a discarded one is not active, and an id the bank never
/// held comes back too, ordered by id.
#[test]
fn postgres_the_claim_state_read_reports_live_discarded_and_absent_sources() {
    let (_guard, store) = pg_store();
    let bank = unique("pg-claim-state");
    let live = unique("live");
    let discarded = unique("discarded");
    let absent = unique("absent");
    for id in [&live, &discarded] {
        store
            .create_memory_item(&stamped(&bank, id, "1767225600000"), "agent")
            .expect("item");
    }
    store
        .discard_item(&bank, &discarded, "test", "1767225600000", "agent")
        .expect("discard");

    let asked = vec![live.clone(), discarded.clone(), absent.clone()];
    let states = store
        .distilled_unit_claim_state(&bank, &asked)
        .expect("claim state pg");
    let by_id: std::collections::BTreeMap<&str, &ClaimedSourceState> =
        states.iter().map(|s| (s.source_id.as_str(), s)).collect();
    assert_eq!(by_id.len(), 3, "every asked id answers");
    assert!(
        by_id[live.as_str()].active,
        "a stored source is in active use"
    );
    assert_eq!(
        by_id[live.as_str()].updated_at.as_deref(),
        Some("1767225600000")
    );
    assert!(
        !by_id[discarded.as_str()].active,
        "a discarded source is not in active use"
    );
    assert!(!by_id[absent.as_str()].active, "an unheld id is not active");
    assert_eq!(by_id[absent.as_str()].updated_at, None, "and has no stamp");
    assert_eq!(
        states
            .iter()
            .map(|s| s.source_id.as_str())
            .collect::<Vec<_>>(),
        vec![absent.as_str(), discarded.as_str(), live.as_str()],
        "ordered by id, so two reads cannot differ"
    );
}

#[test]
fn postgres_the_claim_state_read_of_an_empty_claim_set_is_empty() {
    let (_guard, store) = pg_store();
    let claim_state = store
        .distilled_unit_claim_state("b", &[])
        .expect("empty read");
    assert!(claim_state.is_empty(), "{claim_state:?}");
}
