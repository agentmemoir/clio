// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the operations-removal cascade contract
//! tests: a confirmed `discard` makes every distilled unit claiming the
//! removed item drop it from its claim set and go stale with reason
//! `source_removed`, the record and its snapshot projection stay in agreement,
//! the audited row stays readable with its gist and DEK untouched, and the
//! cascade reaches both backends through the one `discard_item` hook.
//!
//! ## Owns
//! - The backend-agnostic cascade suite plus its per-backend runners
//!
//! ## Does not own
//! - Non-cascade discard contract (`discard_tests`)
//! - Engine-level reconcile and rebuild scenarios (`clio-write` tests)
//! - Recall withholding behavior (`clio-retrieve` tests)
//!
//! ## Boundary
//!
//! Changes to the cascade contract should normally update the suite here.
//! Changes to discard validation or one backend's SQL should not.
//!
//! Keep this module focused on the cascade store contract.
//!
//! Test-only. Never compiled into production builds.

use crate::dek::{DekProvider, LocalDevKms};
use crate::distilled_unit::distilled_unit_reconcile_tests::record;
use crate::distilled_unit::distilled_unit_tests::hub_item;
use crate::distilled_unit::{DistilledUnitRecord, STALE_SOURCE_REMOVED};
use crate::postgres::PostgresStore;
use crate::sqlite::SqliteStore;
use crate::store::Store;
use crate::test_unique::unique;
use std::sync::Arc;

/// The store surface the cascade suite reads through: the discard hook plus a
/// raw audit-operation listing for the cascade's telemetry rows.
trait CascadeBackend: Store {
    /// Operations recorded for one item, oldest first.
    fn audit_ops(&self, item_id: &str) -> Vec<String>;
    /// The subject key row for one subject: `(dek_ref, status)`.
    fn subject_key(&self, subject_id: &str) -> (Option<String>, Option<String>);
}

/// Seed one distilled unit claiming `hub` and `sources`, all under `subject`.
fn seed_unit(
    store: &dyn CascadeBackend,
    bank: &str,
    subject: &str,
    hub: &str,
    sources: &[&str],
) -> (String, DistilledUnitRecord) {
    let item_id = format!("hub-distill:{bank}:{hub}");
    // The claimed items must exist: the discard hook resolves the removed
    // item's identity first.
    for id in std::iter::once(hub).chain(sources.iter().copied()) {
        let item = crate::erase_test_support::memory_item(bank, id, subject);
        store.create_memory_item(&item, "tester").unwrap();
    }
    let item = hub_item(bank, &item_id, subject, hub, sources);
    let mut r = record(
        bank,
        &item_id,
        hub,
        &sources.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>(),
    );
    // The trail a real distillation writes, so the cascade's effect on the
    // current revision's evidence is exercised against stored state.
    let cluster: Vec<_> = std::iter::once(hub)
        .chain(sources.iter().copied())
        .map(|id| store.get_memory_item(bank, id).unwrap().unwrap())
        .collect();
    r.record_evidence(
        crate::distilled_evidence::evidence_from_items(cluster.iter()),
        &r.distilled_at.clone(),
    );
    store.commit_distilled_unit(&item, &r, "tester").unwrap();
    (item_id, r)
}

/// Seed one distilled unit whose own subject differs from its sources': lets a
/// test destroy the unit's key alone and leave the rest of the bank readable.
fn seed_unit_split(
    store: &dyn CascadeBackend,
    bank: &str,
    unit_subject: &str,
    source_subject: &str,
    hub: &str,
    sources: &[&str],
) -> (String, DistilledUnitRecord) {
    let item_id = format!("hub-distill:{bank}:{hub}");
    for id in std::iter::once(hub).chain(sources.iter().copied()) {
        let item = crate::erase_test_support::memory_item(bank, id, source_subject);
        store.create_memory_item(&item, "tester").unwrap();
    }
    let item = hub_item(bank, &item_id, unit_subject, hub, sources);
    let r = record(
        bank,
        &item_id,
        hub,
        &sources.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>(),
    );
    store.commit_distilled_unit(&item, &r, "tester").unwrap();
    (item_id, r)
}

/// The backend-agnostic cascade contract.
fn suite_cascade(store: &dyn CascadeBackend) {
    let bank = unique("cascade");
    let subject = unique("subject");
    let hub = unique("hub");
    let a = unique("source-a");
    let b = unique("source-b");
    let (unit_id, _first_record) = seed_unit(store, &bank, &subject, &hub, &[&a, &b]);
    let gist_before = store.get_gist(&bank, &unit_id).unwrap();

    // Discard one claimed source: the claim set drops it and the unit goes
    // stale with reason `source_removed`.
    store
        .discard_item(&bank, &a, "withdrawn", "2026-06-02T00:00:00Z", "tester")
        .unwrap();
    let after = store.get_distilled_unit(&bank, &unit_id).unwrap().unwrap();
    assert_eq!(after.source_ids, vec![b.clone()]);
    assert_eq!(after.stale_reason.as_deref(), Some(STALE_SOURCE_REMOVED));
    assert_eq!(after.stale_at.as_deref(), Some("2026-06-02T00:00:00Z"));
    assert_eq!(after.revision, 1, "the cascade writes no revision");

    // The current revision's evidence list follows the claim set: a source that
    // left active use stops counting toward the proof count, so the count and
    // the claim set cannot disagree.
    let counted: std::collections::BTreeSet<String> = after
        .current_evidence()
        .iter()
        .map(|e| e.source_id.clone())
        .collect();
    assert_eq!(counted, after.claim_ids(), "{counted:?}");
    assert!(
        !counted.contains(&a),
        "a removed source is not counted as current evidence"
    );

    // Record and snapshot agree after the cascade.
    let snapshot = store
        .get_snapshot(&bank, &after.current_revision_item_id())
        .unwrap()
        .unwrap();
    let mut claimed = std::collections::BTreeSet::new();
    claimed.insert(snapshot["hub_id"].as_str().unwrap().to_owned());
    for id in snapshot["neighbor_ids"].as_array().unwrap() {
        claimed.insert(id.as_str().unwrap().to_owned());
    }
    assert_eq!(after.claim_ids(), claimed);
    assert_eq!(snapshot["stale_reason"], STALE_SOURCE_REMOVED);

    // The audited derived row stays readable, and its gist half is untouched:
    // the cascade edits the claim set, never the text.
    let unit = store
        .get_memory_item(&bank, &after.current_revision_item_id())
        .unwrap()
        .unwrap();
    assert_eq!(unit.gist, gist_before, "the audited row stays readable");

    // The DEK row is untouched by the cascade.
    let (dek_ref, status) = store.subject_key(&subject);
    assert_eq!(status.as_deref(), Some("active"));
    assert!(dek_ref.is_some(), "the DEK row must survive the cascade");

    // The cascade is recorded with ids and counts only.
    let ops = store.audit_ops(&unit_id);
    assert!(
        ops.iter().any(|op| op == "hub_distill_source_removed"),
        "the cascade must record its event: {ops:?}"
    );

    // A second discarded source updates the same unit once, without
    // duplicating entries.
    store
        .discard_item(
            &bank,
            &b,
            "also withdrawn",
            "2026-06-03T00:00:00Z",
            "tester",
        )
        .unwrap();
    let after_two = store.get_distilled_unit(&bank, &unit_id).unwrap().unwrap();
    assert_eq!(after_two.source_ids, Vec::<String>::new());
    assert_eq!(after_two.stale_at.as_deref(), Some("2026-06-03T00:00:00Z"));
    let ops = store.audit_ops(&unit_id);
    assert_eq!(
        ops.iter()
            .filter(|op| op.as_str() == "hub_distill_source_removed")
            .count(),
        2,
        "one cascade event per discard"
    );
    // A discarded hub stays a claim, so its evidence entry and the count stay.
    let discarded = store.discard_item(&bank, &hub, "withdrawn", "2026-06-05", "tester");
    discarded.unwrap();
    let after_hub = store.get_distilled_unit(&bank, &unit_id).unwrap().unwrap();
    assert_eq!(after_hub.proof_count(), after_hub.claim_ids().len());

    // A discarded item no unit claims cascades to nothing.
    let stranger = unique("stranger");
    let stranger_item = crate::erase_test_support::memory_item(&bank, &stranger, &subject);
    store.create_memory_item(&stranger_item, "tester").unwrap();
    let before_all = store.list_distilled_units(&bank).unwrap();
    store
        .discard_item(
            &bank,
            &stranger,
            "irrelevant",
            "2026-06-04T00:00:00Z",
            "tester",
        )
        .unwrap();
    assert_eq!(store.list_distilled_units(&bank).unwrap(), before_all);
}

/// A cascade must tolerate a unit whose content erasure made unreadable: the
/// dead unit's record stays untouched and the discard still succeeds.
#[test]
fn sqlite_cascade_skips_a_dead_unit_and_the_discard_still_succeeds() {
    let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
    let store = SqliteStore::open_in_memory(kms).unwrap();
    let bank = unique("cascade-dead");
    let subject = unique("subject");
    let hub = unique("hub");
    let a = unique("source-a");
    let (unit_id, r) = seed_unit(&store, &bank, &subject, &hub, &[&a]);
    store
        .with_conn(|conn| {
            conn.execute(
                "UPDATE items SET content_ciphertext = NULL WHERE id = ?1",
                [unit_id.as_str()],
            )
            .map_err(|e| clio_types::AmError::new(clio_types::ErrorCode::Internal, e.to_string()))
        })
        .unwrap();
    store
        .discard_item(&bank, &a, "withdrawn", "2026-06-02T00:00:00Z", "tester")
        .unwrap();
    assert_eq!(
        store.get_distilled_unit(&bank, &unit_id).unwrap().unwrap(),
        r,
        "a dead unit's record must stay untouched"
    );
}

/// A cascade must tolerate a unit whose current revision row an operator
/// correction closed: the historical row is never rewritten, the record
/// stays untouched, and the discard still succeeds.
#[test]
fn sqlite_cascade_skips_a_corrected_unit_and_the_discard_still_succeeds() {
    let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
    let store = SqliteStore::open_in_memory(kms).unwrap();
    let bank = unique("cascade-corrected");
    let subject = unique("subject");
    let hub = unique("hub");
    let a = unique("source-a");
    let (unit_id, r) = seed_unit(&store, &bank, &subject, &hub, &[&a]);
    // An operator correction closes the unit's current revision row.
    let ts = "2026-06-01T12:00:00Z";
    let current = store
        .get_memory_item(&bank, &unit_id)
        .unwrap()
        .expect("unit row");
    let mut corrected = current.clone();
    corrected.id = unique("corrected");
    corrected.valid_from = ts.into();
    corrected.tx_from = ts.into();
    corrected.created_at = ts.into();
    corrected.updated_at = ts.into();
    store
        .correct_item(&crate::audit_model::CorrectionSpec {
            bank_id: bank.clone(),
            prior_item_id: unit_id.clone(),
            new_item: corrected,
            reason: "misstated".into(),
            actor: "tester".into(),
            timestamp: ts.into(),
        })
        .unwrap();
    store
        .discard_item(&bank, &a, "withdrawn", "2026-06-02T00:00:00Z", "tester")
        .unwrap();
    assert_eq!(
        store.get_distilled_unit(&bank, &unit_id).unwrap().unwrap(),
        r,
        "a corrected unit's record must stay untouched"
    );
}

/// A cascade must tolerate a unit whose body will not decrypt because its
/// subject key was shredded: one damaged derived row must not reject the
/// operator's discard of an unrelated item, and the dead unit's record must
/// stay untouched as the erasure evidence it is.
#[test]
fn sqlite_cascade_skips_a_shredded_unit_and_the_discard_still_succeeds() {
    let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
    let store = SqliteStore::open_in_memory(kms.clone()).unwrap();
    let bank = unique("cascade-shred");
    let hub = unique("hub");
    let a = unique("source-a");
    // The unit gets its own subject so shredding it leaves the source readable
    // and the discard resolvable.
    let unit_subject = unique("unit-subject");
    let source_subject = unique("source-subject");
    let (unit_id, r) = seed_unit_split(&store, &bank, &unit_subject, &source_subject, &hub, &[&a]);
    // Crypto-shred the unit's subject key while its ciphertext stays in place:
    // the row still exists, and opening it now fails closed.
    kms.destroy_dek(&unit_subject).unwrap();

    store
        .discard_item(&bank, &a, "withdrawn", "2026-06-02T00:00:00Z", "tester")
        .unwrap();
    assert_eq!(
        store.get_distilled_unit(&bank, &unit_id).unwrap().unwrap(),
        r,
        "an undecryptable unit's record must stay untouched"
    );
    assert!(
        store.get_item_identity(&bank, &a).unwrap().is_none(),
        "the discard itself must have landed"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    impl CascadeBackend for SqliteStore {
        fn audit_ops(&self, item_id: &str) -> Vec<String> {
            self.with_conn(|conn| {
                let mut stmt = conn
                    .prepare(
                        "SELECT operation FROM audit_events WHERE item_id = ?1
                         ORDER BY created_at, id",
                    )
                    .map_err(|e| {
                        clio_types::AmError::new(clio_types::ErrorCode::Internal, e.to_string())
                    })?;
                let rows = stmt
                    .query_map([item_id], |r| r.get::<_, String>(0))
                    .map_err(|e| {
                        clio_types::AmError::new(clio_types::ErrorCode::Internal, e.to_string())
                    })?;
                let mut ops = Vec::new();
                for row in rows {
                    ops.push(row.map_err(|e| {
                        clio_types::AmError::new(clio_types::ErrorCode::Internal, e.to_string())
                    })?);
                }
                Ok(ops)
            })
            .unwrap()
        }
        fn subject_key(&self, subject_id: &str) -> (Option<String>, Option<String>) {
            self.with_conn(|conn| {
                conn.query_row(
                    "SELECT dek_ref, status FROM subject_keys WHERE subject_id = ?1",
                    [subject_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .map_err(|e| {
                    clio_types::AmError::new(clio_types::ErrorCode::Internal, e.to_string())
                })
            })
            .unwrap()
        }
    }

    impl CascadeBackend for PostgresStore {
        fn audit_ops(&self, item_id: &str) -> Vec<String> {
            self.with_client(|client| {
                Ok(client
                    .query(
                        "SELECT operation FROM audit_events WHERE item_id = $1
                         ORDER BY created_at, id",
                        &[&item_id],
                    )
                    .expect("audit query")
                    .iter()
                    .map(|r| r.get(0))
                    .collect())
            })
            .unwrap()
        }
        fn subject_key(&self, subject_id: &str) -> (Option<String>, Option<String>) {
            self.with_client(|client| {
                let row = client
                    .query_opt(
                        "SELECT dek_ref, status FROM subject_keys WHERE subject_id = $1",
                        &[&subject_id],
                    )
                    .expect("subject key query");
                Ok(row.map_or((None, None), |r| (r.get(0), r.get(1))))
            })
            .unwrap()
        }
    }

    #[test]
    fn sqlite_discard_cascades_to_claiming_distilled_units() {
        let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
        let store = SqliteStore::open_in_memory(kms).unwrap();
        suite_cascade(&store);
    }

    #[test]
    fn postgres_discard_cascades_to_claiming_distilled_units() {
        let _guard = crate::lock_pg_test();
        let url = crate::db_url::postgres_url_from_env();
        let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
        let store = PostgresStore::open(url.as_str(), kms).unwrap();
        suite_cascade(&store);
    }

    #[test]
    fn postgres_cascade_skips_a_shredded_unit_and_the_discard_still_succeeds() {
        let _guard = crate::lock_pg_test();
        let url = crate::db_url::postgres_url_from_env();
        let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
        let store = PostgresStore::open(url.as_str(), kms.clone()).unwrap();
        let bank = unique("pg-cascade-shred");
        let hub = unique("hub");
        let a = unique("source-a");
        let unit_subject = unique("unit-subject");
        let source_subject = unique("source-subject");
        let (unit_id, r) =
            seed_unit_split(&store, &bank, &unit_subject, &source_subject, &hub, &[&a]);
        kms.destroy_dek(&unit_subject).unwrap();
        store
            .discard_item(&bank, &a, "withdrawn", "2026-06-02T00:00:00Z", "tester")
            .unwrap();
        assert_eq!(
            store.get_distilled_unit(&bank, &unit_id).unwrap().unwrap(),
            r,
            "an undecryptable unit's record must stay untouched"
        );
        assert!(
            store.get_item_identity(&bank, &a).unwrap().is_none(),
            "the discard itself must have landed"
        );
    }
}
