// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for Postgres parity coverage of the sealed
//! optional belief `context`: create/replace round-trips, hash-only
//! audit telemetry, invalid-context fail-closed writes, and reads that tolerate
//! an unreadable (crypto-shredded) context cipher.
//!
//! ## Owns
//! - Shared-Postgres tests for belief context storage and audit telemetry
//!
//! ## Does not own
//! - The SQLite context tests (see `belief_tests`)
//! - Postgres belief SQL itself (see `postgres_belief`, `belief_tx`)
//!
//! ## Boundary
//!
//! Changes to Postgres belief context behavior should update tests here.
//! Changes to the backend-agnostic contract should go through `belief_tests`.
//!
//! Keep this module focused on shared-Postgres belief context coverage.

use crate::belief_store::belief_store_belief_tests::mk_belief;
use crate::belief_store::{BELIEF_CONTEXT_HASH_DOMAIN, BeliefStore};
use crate::dek::{DekProvider, LocalDevKms};
use crate::migrate::IntoAmError;
use crate::postgres::PostgresStore;
use crate::test_unique::unique;
use clio_types::{CONTEXT_MAX_BYTES, ConfidenceEntry, ErrorCode, SourceType, proposition_key};
use std::sync::Arc;

/// Open the shared Postgres for one test and hand back the database guard.
///
/// The guard is returned, not taken here on purpose. `let _guard =
/// lock_pg_test()` inside this helper would be dropped the moment the helper
/// returned, so every caller would run its whole body against the shared
/// database with no lock: no in-process mutex and no cross-process advisory
/// lock. That is how this file's tests came to deadlock against a store open
/// elsewhere in the workspace, whose DDL needs an exclusive table lock.
///
/// Callers must bind all three values, so the guard lives as long as the test:
///
/// ```ignore
/// let (_guard, store, _kms) = open_pg();
/// ```
pub(crate) fn open_pg() -> (crate::PgTestGuard, PostgresStore, Arc<dyn DekProvider>) {
    let guard = crate::lock_pg_test();
    let url = crate::db_url::postgres_url_from_env();
    assert!(url.starts_with("postgres"));
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    let store = PostgresStore::open(&url, Arc::clone(&kms))
        .unwrap_or_else(|e| panic!("postgres+pgvector required (compose up): {e}"));
    (guard, store, kms)
}

fn second_entry() -> ConfidenceEntry {
    ConfidenceEntry::new("2026-07-14", 0.65, "turn_340", SourceType::AgentInferred).expect("entry")
}

/// Create/append with context on Postgres: reads surface the exact sealed
/// value, plain appends preserve it, context appends replace it, and the audit
/// rows carry hash-only context telemetry (never the raw value).
#[test]
#[allow(clippy::too_many_lines)]
fn postgres_belief_context_create_append_and_audit_parity() {
    let (_guard, store, _kms) = open_pg();
    let bank = unique("pg-bctx-bank");
    let id = unique("pg-bctx");
    let mut belief = mk_belief(&id, &bank, &unique("pg-bctx-subj"));
    belief.context = Some("prefer oat milk at home".into());
    store.create_belief(&belief, "agent").unwrap();
    let want = Some("prefer oat milk at home".to_owned());
    assert_eq!(
        store.get_belief(&bank, &id).unwrap().expect("get").context,
        want
    );
    assert!(
        store
            .list_beliefs(&bank)
            .unwrap()
            .iter()
            .any(|b| b.id == id && b.context == want)
    );
    let key = proposition_key(&belief.proposition);
    assert_eq!(
        store
            .find_belief_by_proposition(&bank, &key)
            .unwrap()
            .expect("found")
            .context,
        want
    );

    let second = second_entry();
    store
        .append_confidence(&bank, &id, &second, "agent")
        .unwrap();
    let after_plain = store.get_belief(&bank, &id).unwrap().expect("get");
    assert_eq!(after_plain.context, want, "plain append must preserve");
    // A differing append context is rejected before any write.
    let err = store
        .append_confidence_with_context(&bank, &id, &second, "agent", Some("new context"))
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    // A byte-identical context is accepted and preserves the stored value.
    store
        .append_confidence_with_context(
            &bank,
            &id,
            &second,
            "agent",
            Some("prefer oat milk at home"),
        )
        .unwrap();
    let after_identical = store.get_belief(&bank, &id).unwrap().expect("get");
    assert_eq!(
        after_identical.context.as_deref(),
        Some("prefer oat milk at home")
    );

    let rows: Vec<String> = store
        .with_client(|client| {
            Ok(client
                .query(
                    "SELECT detail_json FROM audit_events
                     WHERE operation = 'belief_observe' AND item_id = $1
                     ORDER BY created_at, id",
                    &[&id],
                )
                .am("audit query")?
                .iter()
                .map(|r| r.get(0))
                .collect())
        })
        .unwrap();
    assert_eq!(rows.len(), 3);
    let create: serde_json::Value = serde_json::from_str(&rows[0]).unwrap();
    assert_eq!(create["created"], serde_json::json!(true));
    assert_eq!(create["context_present"], serde_json::json!(true));
    assert_eq!(
        create["context_len"],
        belief.context.as_ref().unwrap().len()
    );
    assert_eq!(
        create["context_hash"],
        crate::sha256_hex(BELIEF_CONTEXT_HASH_DOMAIN, belief.context.as_ref().unwrap(),)
    );
    // Plain append preserved the context: no context telemetry was added.
    let plain: serde_json::Value = serde_json::from_str(&rows[1]).unwrap();
    assert!(plain.get("context_present").is_none(), "{plain}");
    let observed: serde_json::Value = serde_json::from_str(&rows[2]).unwrap();
    assert_eq!(observed["appended"], serde_json::json!(true));
    assert_eq!(observed["context_present"], serde_json::json!(true));
    assert_eq!(observed["context_len"], "prefer oat milk at home".len());
    assert_eq!(
        observed["context_hash"],
        crate::sha256_hex(BELIEF_CONTEXT_HASH_DOMAIN, "prefer oat milk at home")
    );
    let all = rows.join("\n");
    assert!(!all.contains("prefer oat milk"), "{all}");
    assert!(!all.contains("new context"), "{all}");
}

/// A context sealed under a DEK that is later crypto-shredded must not fail
/// the Postgres belief read: the read succeeds and surfaces `context: None`.
#[test]
fn postgres_belief_context_unreadable_reads_none() {
    let (_guard, store, kms) = open_pg();
    let bank = unique("pg-shred-bank");
    let id = unique("pg-shred");
    let mut b = mk_belief(&id, &bank, &unique("pg-shred-subj"));
    b.context = Some("secret context".into());
    store.create_belief(&b, "agent").unwrap();
    // Re-seal the context under a second subject, then destroy its DEK.
    let shred = unique("pg-shred-ctx-subj");
    let ghost = kms
        .as_ref()
        .encrypt(&shred, b"ghost context")
        .unwrap()
        .to_storage_string();
    store
        .with_client(|client| {
            client
                .execute(
                    "UPDATE beliefs SET context_cipher = $1 WHERE id = $2",
                    &[&ghost, &id],
                )
                .am("context tamper")
        })
        .unwrap();
    kms.destroy_dek(&shred).unwrap();
    let got = store
        .get_belief(&bank, &id)
        .unwrap()
        .expect("read succeeds");
    assert_eq!(got.proposition, b.proposition);
    assert_eq!(got.context, None);
    assert!(
        store
            .list_beliefs(&bank)
            .unwrap()
            .iter()
            .any(|x| x.id == id && x.context.is_none())
    );
}

/// Oversized and empty contexts fail closed on Postgres before any write,
/// on both the create and the context-replace append path.
#[test]
fn postgres_belief_context_rejects_invalid_before_write() {
    let (_guard, store, _kms) = open_pg();
    let bank = unique("pg-bad-bank");
    let id = unique("pg-bad");
    let mut b = mk_belief(&id, &bank, &unique("pg-bad-subj"));
    b.context = Some(String::new());
    assert_eq!(
        store.create_belief(&b, "agent").unwrap_err().code,
        ErrorCode::InvalidArgument
    );
    b.id = unique("pg-bad-big");
    b.context = Some("x".repeat(CONTEXT_MAX_BYTES + 1));
    assert_eq!(
        store.create_belief(&b, "agent").unwrap_err().code,
        ErrorCode::OutOfRange
    );
    let rejected_beliefs = store.list_beliefs(&bank).unwrap();
    assert!(rejected_beliefs.is_empty(), "{rejected_beliefs:?}");
    // The append path validates the context before any write too.
    let valid_id = unique("pg-bad-valid");
    let mut valid = mk_belief(&valid_id, &bank, &unique("pg-bad-subj"));
    valid.context = Some("kept context".into());
    store.create_belief(&valid, "agent").unwrap();
    let second = second_entry();
    let err = store
        .append_confidence_with_context(
            &bank,
            &valid_id,
            &second,
            "agent",
            Some(&"x".repeat(CONTEXT_MAX_BYTES + 1)),
        )
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::OutOfRange);
    let got = store.get_belief(&bank, &valid_id).unwrap().expect("get");
    assert_eq!(got.context.as_deref(), Some("kept context"));
}

/// Postgres parity for the authorized belief-context correction: the sealed
/// context is swapped, the trajectory is untouched, and the `correct` audit
/// row carries the reason and prior/new hash-only stats only (the
/// sealed prior value is never retained; raw plaintext never enters telemetry).
#[test]
fn postgres_belief_context_correction_parity() {
    let (_guard, store, _kms) = open_pg();
    let bank = unique("pg-bcorr-bank");
    let id = unique("pg-bcorr");
    let mut b = mk_belief(&id, &bank, &unique("pg-bcorr-subj"));
    b.context = Some("original context".into());
    store.create_belief(&b, "agent").unwrap();

    store
        .correct_belief_context(&bank, &id, "corrected context", "operator fix", "agent")
        .unwrap();
    let got = store.get_belief(&bank, &id).unwrap().expect("belief");
    assert_eq!(got.context.as_deref(), Some("corrected context"));
    assert_eq!(got.confidence_history.len(), 1);
    assert_eq!(got.confidence_history[0], b.confidence_history[0]);

    let row: (String, Option<f64>, String) = store
        .with_client(|client| {
            let r = client
                .query_one(
                    "SELECT operation, confidence::float8, detail_json FROM audit_events
                     WHERE operation = 'correct' AND item_id = $1",
                    &[&id],
                )
                .am("audit query")?;
            Ok((r.get(0), r.get(1), r.get(2)))
        })
        .unwrap();
    assert_eq!(row.0, "correct");
    // A correction observes nothing: the row carries no confidence.
    assert!(
        row.1.is_none(),
        "the correction row must not fabricate a confidence"
    );
    let value: serde_json::Value = serde_json::from_str(&row.2).unwrap();
    assert_eq!(value["corrected"], serde_json::json!("context"));
    assert_eq!(value["reason"], serde_json::json!("operator fix"));
    assert_eq!(value["prior_context_present"], serde_json::json!(true));
    assert_eq!(value["prior_context_len"], "original context".len());
    assert_eq!(
        value["prior_context_hash"],
        crate::sha256_hex(BELIEF_CONTEXT_HASH_DOMAIN, "original context")
    );
    assert_eq!(value["context_len"], "corrected context".len());
    assert!(!row.2.contains("original context"));
    assert!(!row.2.contains("corrected context"));
    // The sealed prior cipher is not retained in the audit row, so the
    // prior context is unrecoverable by design.
    assert!(
        value.get("prior_context_cipher").is_none(),
        "the sealed prior context must not be stored: {}",
        row.2
    );

    // Fail-closed inputs keep the corrected value untouched.
    let err = store
        .correct_belief_context(&bank, &id, "", "r", "agent")
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    let err = store
        .correct_belief_context(&bank, &unique("pg-bcorr-missing"), "v", "r", "agent")
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    let got = store.get_belief(&bank, &id).unwrap().expect("belief");
    assert_eq!(got.context.as_deref(), Some("corrected context"));
}

/// Belief create/append validation errors keep their structured codes on
/// Postgres parity (shared with `belief_tests::run_belief_suite` shape).
#[test]
fn postgres_belief_context_audit_detail_is_pii_safe_json() {
    let (_guard, store, _kms) = open_pg();
    let bank = unique("pg-pii-bank");
    let id = unique("pg-pii");
    let mut b = mk_belief(&id, &bank, &unique("pg-pii-subj"));
    b.context = Some("bank detail".into());
    store.create_belief(&b, "agent").unwrap();
    let detail: String = store
        .with_client(|client| {
            Ok(client
                .query_one(
                    "SELECT detail_json FROM audit_events
                     WHERE operation = 'belief_observe' AND item_id = $1
                       AND confidence = $2::float8",
                    &[&id, &0.4],
                )
                .am("audit row")?
                .get(0))
        })
        .unwrap();
    let value: serde_json::Value = serde_json::from_str(&detail).unwrap();
    assert_eq!(value["entry_count"], 1);
    assert_eq!(value["context_present"], serde_json::json!(true));
    assert!(!detail.contains("bank detail"));
}

fn read_beliefs_row(store: &PostgresStore, bank: &str, id: &str) -> (String, Option<String>) {
    store
        .with_client(|client| {
            let row = client
                .query_one(
                    "SELECT updated_at, context_as_of FROM beliefs WHERE id = $1 AND bank_id = $2",
                    &[&id, &bank],
                )
                .am("beliefs stamp row")?;
            Ok((row.get(0), row.get(1)))
        })
        .unwrap()
}

/// On Postgres: an append older than the correction stamp must not
/// regress `updated_at`, and the change feed must still select the row.
#[test]
fn postgres_belief_change_stamp_never_regresses_past_older_appends() {
    use crate::SyncStore as _;
    let (_guard, store, _kms) = open_pg();
    let bank = unique("pg-bmono-bank");
    let id = unique("pg-bmono");
    let mut b = mk_belief(&id, &bank, &unique("pg-bmono-subj"));
    b.context = Some("original context".into());
    store.create_belief(&b, "agent").unwrap();
    let stamp_after_create = read_beliefs_row(&store, &bank, &id).0;

    store
        .correct_belief_context(&bank, &id, "corrected context", "operator fix", "agent")
        .unwrap();
    let stamp_after_correct = read_beliefs_row(&store, &bank, &id).0;
    assert!(stamp_after_correct.as_str() > stamp_after_create.as_str());

    let late = ConfidenceEntry::new("2026-01-01", 0.55, "turn_late", SourceType::AgentInferred)
        .expect("entry");
    store.append_confidence(&bank, &id, &late, "agent").unwrap();
    assert_eq!(
        read_beliefs_row(&store, &bank, &id).0,
        stamp_after_correct,
        "an older `as_of` must not regress beliefs.updated_at"
    );

    let feed = store
        .feed_beliefs(&bank, "2026-06-02T00:00:00Z", 10)
        .unwrap();
    assert_eq!(feed.len(), 1, "the row must stay selected: {feed:?}");
    assert_eq!(feed[0].0, stamp_after_correct);
    assert_eq!(feed[0].1.context.as_deref(), Some("corrected context"));
}

/// on Postgres: `context_as_of` is store-derived — `created_at` at
/// create, the correction stamp after a correction, NULL without a context.
#[test]
fn postgres_belief_context_as_of_is_store_derived() {
    let (_guard, store, _kms) = open_pg();
    let bank = unique("pg-bstamp-bank");
    let id = unique("pg-bstamp");
    let mut b = mk_belief(&id, &bank, &unique("pg-bstamp-subj"));
    b.context = Some("original context".into());
    store.create_belief(&b, "agent").unwrap();
    let got = store.get_belief(&bank, &id).unwrap().expect("belief");
    assert_eq!(got.context_as_of.as_deref(), Some(b.created_at.as_str()));

    store
        .correct_belief_context(&bank, &id, "corrected context", "operator fix", "agent")
        .unwrap();
    let (updated_at, context_as_of) = read_beliefs_row(&store, &bank, &id);
    let stamp = context_as_of.expect("correction stamps the effective time");
    assert!(stamp.as_str() >= b.created_at.as_str());
    assert_eq!(updated_at, stamp);

    // A caller-supplied stamp is ignored: the store derives it.
    let forged = unique("pg-bstamp-forged");
    let mut f = mk_belief(&forged, &bank, &unique("pg-bstamp-subj"));
    f.proposition = "User Prefers Rooibos Tea".into();
    f.context = Some("forged context".into());
    f.context_as_of = Some("2020-01-01".into());
    store.create_belief(&f, "agent").unwrap();
    let got = store.get_belief(&bank, &forged).unwrap().expect("belief");
    assert_eq!(got.context_as_of.as_deref(), Some(f.created_at.as_str()));

    // Create without a context leaves the stamp NULL.
    let bare = unique("pg-bstamp-bare");
    let mut none = mk_belief(&bare, &bank, &unique("pg-bstamp-subj"));
    none.proposition = "User Prefers Chamomile Tea".into();
    none.context = None;
    store.create_belief(&none, "agent").unwrap();
    let got = store.get_belief(&bank, &bare).unwrap().expect("belief");
    assert_eq!(got.context, None);
    assert_eq!(got.context_as_of, None);
}
