// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the per-revision evidence trail on a
//! distilled unit: entries hold ids and timestamps only, one list per revision,
//! re-recording a revision does not grow the trail, a no-op write appends
//! nothing, and the whole trail survives a store reopen.
//!
//! ## Owns
//! - The evidence scenarios: entry shape, count derivation, per-revision
//!   append-only, idempotence, and durability
//!
//! ## Does not own
//! - The freshness decision (`distilled_freshness`)
//! - The Postgres backend's identical behaviour (`postgres_distilled_unit_tests`)
//!
//! ## Boundary
//!
//! Changes to how a unit's evidence is recorded should normally require changes
//! here. Changes to the staleness decision should require
//! `distilled_freshness_tests`; changes to Postgres scenarios should require
//! `postgres_distilled_unit_tests`.
//!
//! Keep this module focused on the evidence trail.
//!
//! Test-only. Never compiled into production builds.

use crate::distilled_evidence::{EvidenceEntry, RevisionEvidence, evidence_from_items};
use crate::distilled_unit::distilled_unit_tests::hub_item;
use crate::distilled_unit::{DISTILLED_UNIT_RECORD_VERSION, DistilledUnitRecord};
use crate::erase_test_support::{local_kms, memory_item, unique};
use crate::migrate::IntoAmError;
use crate::sqlite::SqliteStore;
use crate::store::Store as _;
use clio_types::MemoryItem;

/// An item with an explicit `updated_at`, so evidence assertions never depend on
/// when the suite ran.
fn stamped(id: &str, updated_at: &str) -> MemoryItem {
    let mut item = memory_item("b", id, "subj");
    item.updated_at = updated_at.to_owned();
    item
}

/// A record at `revision` over the given sources.
fn record_at(revision: u32, sources: &[&str]) -> DistilledUnitRecord {
    let mut r = DistilledUnitRecord::new(
        "b",
        "u",
        "h",
        sources.iter().map(|s| (*s).to_owned()),
        "hub_distilled",
        "2026-06-01T00:00:00Z",
    );
    r.revision = revision;
    r
}

/// The entry shape is exactly a source id and its last-change time. A field that
/// could hold text would make the record a copy of the content it refers to.
#[test]
fn an_evidence_entry_carries_an_id_and_a_time_and_nothing_else() {
    let entry = EvidenceEntry::new("s1", Some("2026-06-01T00:00:00Z")).expect("valid entry");
    let value = serde_json::to_value(&entry).expect("entry serializes");
    let object = value.as_object().expect("entry is an object");
    let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        ["as_of", "source_id"],
        "no text or gist field may exist"
    );
    assert!(
        object.values().all(|v| v.is_string() || v.is_null()),
        "an entry value is an id or a time: {value}"
    );
    assert!(
        EvidenceEntry::new("", Some("2026-06-01T00:00:00Z")).is_err(),
        "an entry without a source id is not an entry"
    );
}

/// A cluster of items becomes one entry per distinct source, ordered by id, each
/// carrying that item's own `updated_at`. Ordering is the point: it is what
/// makes a proof count and a list comparison independent of scan order.
#[test]
fn a_cluster_becomes_one_ordered_entry_per_distinct_source() {
    let hub = stamped("h", "2026-05-01T00:00:00Z");
    let sources = [
        stamped("s2", "2026-05-02T00:00:00Z"),
        stamped("s1", "2026-05-03T00:00:00Z"),
        stamped("s2", "2026-05-02T00:00:00Z"),
    ];
    let entries = evidence_from_items(std::iter::once(&hub).chain(sources.iter()));
    let ids: Vec<&str> = entries.iter().map(|e| e.source_id.as_str()).collect();
    assert_eq!(ids, ["h", "s1", "s2"], "sorted by id, each source once");
    assert_eq!(
        entries[1].as_of.as_deref(),
        Some("2026-05-03T00:00:00Z"),
        "the entry carries the source's own last-change time"
    );
    assert_eq!(entries.len(), 3, "a repeated id is counted once");
}

/// An item the write path never stamped yields an explicit absent time, not an
/// invented one and not a dropped source.
#[test]
fn an_unstamped_source_is_recorded_with_an_absent_time() {
    let mut bare = memory_item("b", "s1", "subj");
    bare.updated_at = String::new();
    let entries = evidence_from_items([&bare]);
    assert_eq!(entries.len(), 1, "the source is still a source");
    assert_eq!(entries[0].as_of, None, "with no invented time");
}

/// A first distillation records one list for revision 1 and the proof count is
/// its length.
#[test]
fn the_proof_count_is_the_current_revisions_distinct_sources() {
    let mut r = record_at(1, &["s1", "s2"]);
    assert_eq!(r.proof_count(), 0, "no evidence yet means no proof");
    r.record_evidence(
        evidence_from_items([
            &stamped("h", "2026-05-01T00:00:00Z"),
            &stamped("s1", "2026-05-02T00:00:00Z"),
            &stamped("s2", "2026-05-03T00:00:00Z"),
        ]),
        "2026-06-01T00:00:00Z",
    );
    assert_eq!(r.proof_count(), 3, "hub plus two sources");
    assert_eq!(r.current_evidence().len(), 3);
    assert_eq!(r.evidence_history().len(), 1, "one revision so far");
    r.validate().expect("a record with one valid list is valid");
}

/// A reconcile appends a second list and leaves the first exactly as it was. The
/// proof count follows the current revision; the history keeps both.
#[test]
fn a_later_revision_appends_and_never_rewrites_an_earlier_list() {
    let mut r = record_at(1, &["s1"]);
    r.record_evidence(
        evidence_from_items([&stamped("s1", "2026-05-02T00:00:00Z")]),
        "t1",
    );
    let first = r.evidence_history().to_vec();

    r.revision = 2;
    r.record_evidence(
        evidence_from_items([
            &stamped("s1", "2026-05-09T00:00:00Z"),
            &stamped("s2", "2026-05-09T00:00:00Z"),
        ]),
        "t2",
    );
    assert_eq!(r.evidence_history()[0], first[0], "revision 1 is untouched");
    assert_eq!(r.evidence_history().len(), 2, "history is not rewritten");
    assert_eq!(r.proof_count(), 2, "the count follows the current revision");
    assert_eq!(
        r.current_evidence()[0].as_of.as_deref(),
        Some("2026-05-09T00:00:00Z"),
        "the current list is the new one"
    );
}

/// Re-recording the same revision replaces that revision's list in place. A
/// retried write must not grow a second copy of it or inflate the count.
#[test]
fn re_recording_one_revision_replaces_it_instead_of_appending() {
    let mut r = record_at(1, &["s1"]);
    r.record_evidence(
        evidence_from_items([&stamped("s1", "2026-05-02T00:00:00Z")]),
        "t1",
    );
    r.record_evidence(
        evidence_from_items([&stamped("s1", "2026-05-02T00:00:00Z")]),
        "t1",
    );
    assert_eq!(r.evidence_history().len(), 1, "one list per revision");
    assert_eq!(r.proof_count(), 1, "a retry cannot double the proof count");
    r.validate().expect("still valid");
}

/// A wave that writes nothing appends nothing: a record read back unchanged is
/// still one list, and its count is unchanged.
#[test]
fn a_no_op_wave_appends_nothing() {
    let mut r = record_at(1, &["s1", "s2"]);
    r.record_evidence(
        evidence_from_items([
            &stamped("s1", "2026-05-02T00:00:00Z"),
            &stamped("s2", "2026-05-03T00:00:00Z"),
        ]),
        "t1",
    );
    let before = r.clone();
    assert_eq!(before, r, "an untouched record is byte-identical");
    assert_eq!(r.proof_count(), 2);
    assert_eq!(r.evidence_history().len(), 1);
}

/// A trail that repeats a revision, or names one past the record's own, is
/// refused: the proof count reads exactly one list, so two lists for one
/// revision would make it ambiguous.
#[test]
fn a_malformed_trail_is_refused() {
    let mut r = record_at(2, &["s1"]);
    r.record_evidence(
        evidence_from_items([&stamped("s1", "2026-05-02T00:00:00Z")]),
        "t2",
    );
    assert!(r.validate().is_ok(), "one list for revision 2 is valid");

    let mut beyond = r.clone();
    beyond.record_evidence(
        evidence_from_items([&stamped("s1", "2026-05-02T00:00:00Z")]),
        "t2",
    );
    beyond
        .evidence
        .push(crate::distilled_evidence::RevisionEvidence {
            revision: 3,
            reconciled_at: "t3".to_owned(),
            entries: evidence_from_items([&stamped("s1", "2026-05-02T00:00:00Z")]),
        });
    assert!(
        beyond.validate().is_err(),
        "a list for a revision beyond the record's own is refused"
    );
}

/// A stored trail round-trips through the column and survives a reopen, and a
/// row written before the column existed reads as an empty trail rather than
/// failing.
#[test]
fn the_trail_round_trips_and_a_pre_v3_row_reads_empty() {
    let bank = unique("evidence-roundtrip");
    let item_id = format!("hub-distill:{bank}:h");
    {
        let store = SqliteStore::open_in_memory(local_kms()).expect("sqlite");
        let item = hub_item(&bank, &item_id, "subj", "h", &["s1", "s2"]);
        let mut r = DistilledUnitRecord::new(
            &bank,
            &item_id,
            "h",
            ["s1".to_owned(), "s2".to_owned()],
            "hub_distilled",
            "2026-06-01T00:00:00Z",
        );
        r.record_evidence(
            evidence_from_items([
                &stamped("s1", "2026-05-02T00:00:00Z"),
                &stamped("s2", "2026-05-03T00:00:00Z"),
            ]),
            "2026-06-01T00:00:00Z",
        );
        assert_eq!(r.record_version, DISTILLED_UNIT_RECORD_VERSION);
        store
            .commit_distilled_unit(&item, &r, "tester")
            .expect("commit");

        let read = store
            .get_distilled_unit(&bank, &item_id)
            .expect("read")
            .expect("record");
        assert_eq!(read.evidence, r.evidence, "the trail round-trips");
        assert_eq!(read.proof_count(), 2);

        // A pre-v3 row: the column default with no trail in it.
        store
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE distilled_units SET evidence_json = '[]' WHERE bank_id = ?1",
                    [&bank],
                )
                .am("clear the stored trail")
            })
            .expect("clear the trail");
        let pre_v3 = store
            .get_distilled_unit(&bank, &item_id)
            .expect("read")
            .expect("record");
        assert!(
            pre_v3.evidence.is_empty(),
            "a row with no trail reads empty, not broken"
        );
        assert_eq!(pre_v3.proof_count(), 0, "and claims no proof");
    }
}

/// A file-backed store reopens over its own trail. The read needs no DEK, which
/// is the same property the record already had.
#[test]
fn the_trail_survives_a_reopen() {
    let path = std::env::temp_dir().join(format!("am-evidence-{}.db", unique("reopen")));
    let item_id = {
        let store = SqliteStore::open(&path, local_kms()).expect("open file store");
        let bank = "b";
        let id = "hub-distill:b:h".to_owned();
        let item = hub_item(bank, &id, "subj", "h", &["s1"]);
        let mut r = DistilledUnitRecord::new(
            bank,
            &id,
            "h",
            ["s1".to_owned()],
            "hub_distilled",
            "2026-06-01T00:00:00Z",
        );
        r.record_evidence(
            evidence_from_items([&stamped("s1", "2026-05-02T00:00:00Z")]),
            "2026-06-01T00:00:00Z",
        );
        store
            .commit_distilled_unit(&item, &r, "tester")
            .expect("commit");
        id
    };
    let reopened = SqliteStore::open(&path, local_kms()).expect("reopen");
    let read = reopened
        .get_distilled_unit("b", &item_id)
        .expect("read")
        .expect("record");
    assert_eq!(read.proof_count(), 1);
    assert_eq!(
        read.current_evidence()[0].as_of.as_deref(),
        Some("2026-05-02T00:00:00Z")
    );
    drop(reopened);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("db.kms.json"));
}

/// A list for revision 0 names no revision, so the proof count has nothing to
/// read it against.
#[test]
fn a_revision_list_without_a_revision_is_refused() {
    let list = RevisionEvidence {
        revision: 0,
        reconciled_at: "2026-06-01T00:00:00Z".to_owned(),
        entries: vec![EvidenceEntry::new("s1", None::<String>).expect("entry")],
    };
    assert!(list.validate().is_err(), "revision 0 is not a revision");
}

/// A list with no reconciliation time cannot say when its claim set held.
#[test]
fn a_revision_list_without_a_reconciliation_time_is_refused() {
    let list = RevisionEvidence {
        revision: 1,
        reconciled_at: String::new(),
        entries: vec![EvidenceEntry::new("s1", None::<String>).expect("entry")],
    };
    assert!(list.validate().is_err(), "an unclaimed time is not a time");
}

/// One source named twice in one revision would let a single item inflate a
/// proof count, so the list is refused instead of deduplicated at read time.
#[test]
fn a_revision_list_naming_one_source_twice_is_refused() {
    let list = RevisionEvidence {
        revision: 1,
        reconciled_at: "2026-06-01T00:00:00Z".to_owned(),
        entries: vec![
            EvidenceEntry::new("s1", None::<String>).expect("entry"),
            EvidenceEntry::new("s1", None::<String>).expect("entry"),
        ],
    };
    assert!(
        list.validate().is_err(),
        "one source counts once, or not at all"
    );
}

/// An item with no id cannot be named as a source, so it is not evidence. It is
/// skipped rather than stored as an entry with an empty id.
#[test]
fn an_item_without_an_id_is_not_a_source() {
    let mut anonymous = memory_item("b", "s1", "subj");
    anonymous.id = String::new();
    let named = stamped("s2", "2026-05-02T00:00:00Z");
    let entries = evidence_from_items([&anonymous, &named]);
    let ids: Vec<&str> = entries.iter().map(|e| e.source_id.as_str()).collect();
    assert_eq!(ids, ["s2"], "only a named item is a source");
}

/// A committed unit's own stored trail is what a later read sees: the item and
/// the record agree, and the snapshot names the record they belong to.
#[test]
fn a_committed_units_trail_is_readable_through_the_store() {
    let store = SqliteStore::open_in_memory(local_kms()).expect("sqlite");
    let bank = "b";
    let id = "hub-distill:b:h";
    let item = hub_item(bank, id, "subj", "h", &["s1"]);
    let mut r = DistilledUnitRecord::new(
        bank,
        id,
        "h",
        ["s1".to_owned()],
        "hub_distilled",
        "2026-06-01T00:00:00Z",
    );
    r.record_evidence(
        evidence_from_items([&stamped("s1", "2026-05-02T00:00:00Z")]),
        "2026-06-01T00:00:00Z",
    );
    store
        .commit_distilled_unit(&item, &r, "tester")
        .expect("commit");

    let read = store
        .get_distilled_unit(bank, id)
        .expect("read")
        .expect("row");
    assert_eq!(read.proof_count(), 1);
}
