// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for SQLite-only belief-`context` tests: the
//! source-`context` tamper read, `belief_store` helper unit tests (moved here
//! to keep that module within the source-size budget), and the authorized
//! belief-context correction store path.
//!
//! ## Owns
//! - `sqlite_unreadable_context_reads_none_and_succeeds`
//! - `belief_store` helper unit tests (validation, ids, seal/open, stats)
//! - `correct_belief_context` assertions: value swap, fail-closed inputs, and
//!   audit-row reconstruction fields
//!
//! ## Does not own
//! - Shared belief parity suite (`belief_tests`)
//! - Postgres context parity (`belief_context_pg_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::belief_store::belief_store_belief_tests::mk_belief;
use crate::belief_store::{
    BeliefStore, belief_audit_detail, belief_context_correction_detail, belief_context_stats,
    map_belief_insert, open_belief_context, require_nonempty, seal_belief_context, unique_entry_id,
    validate_context_arg,
};
use crate::dek::{DekProvider, LocalDevKms};
use crate::migrate::IntoAmError;
use crate::sqlite::SqliteStore;
use crate::test_unique::unique;
use clio_types::{ConfidenceEntry, ErrorCode, SourceType};
use std::sync::Arc;

/// Store-level gate: `append_confidence_with_context` is an append, never
/// a context mutation. A supplied context MUST be byte-identical to the stored
/// value (supplying one where none is stored counts as a change); anything else
/// fails closed with `InvalidArgument` before any write. Only the authorized
/// `correct_belief_context` operation may replace a stored context.
#[test]
fn sqlite_belief_append_differing_context_fails_closed_before_write() {
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    let store = SqliteStore::open_in_memory(Arc::clone(&kms)).unwrap();
    let bank = unique("bank-bgate");
    let id = unique("bel-bgate");
    let mut b = mk_belief(&id, &bank, &unique("subj-bgate"));
    b.context = Some("stored context".into());
    store.create_belief(&b, "agent").unwrap();
    let second =
        ConfidenceEntry::new("2026-07-14", 0.65, "turn_340", SourceType::AgentInferred).unwrap();

    // A differing supplied context is rejected before any write.
    let err = store
        .append_confidence_with_context(&bank, &id, &second, "agent", Some("different context"))
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    let got = store.get_belief(&bank, &id).unwrap().expect("belief");
    assert_eq!(got.context.as_deref(), Some("stored context"));
    assert_eq!(
        got.confidence_history.len(),
        1,
        "a rejected context change must not append"
    );

    // Supplying a context where none is stored is also a context change.
    let bare = unique("bel-bgate-none");
    let mut none = mk_belief(&bare, &bank, &unique("subj-bgate"));
    none.proposition = "User Is Likely Vegan".into();
    none.context = None;
    store.create_belief(&none, "agent").unwrap();
    let err = store
        .append_confidence_with_context(&bank, &bare, &second, "agent", Some("new context"))
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert_eq!(
        store
            .get_belief(&bank, &bare)
            .unwrap()
            .expect("belief")
            .context,
        None
    );

    // A byte-identical supplied context is accepted and preserves the cipher.
    store
        .append_confidence_with_context(&bank, &id, &second, "agent", Some("stored context"))
        .expect("byte-identical context accepted");
    let got = store.get_belief(&bank, &id).unwrap().expect("belief");
    assert_eq!(got.context.as_deref(), Some("stored context"));
    assert_eq!(got.confidence_history.len(), 2);

    // Only the authorized operation replaces a stored context.
    store
        .correct_belief_context(&bank, &id, "replaced context", "operator fix", "agent")
        .unwrap();
    let got = store.get_belief(&bank, &id).unwrap().expect("belief");
    assert_eq!(got.context.as_deref(), Some("replaced context"));
}

#[test]
fn sqlite_unreadable_context_reads_none_and_succeeds() {
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    let store = SqliteStore::open_in_memory(Arc::clone(&kms)).unwrap();
    let bank = unique("bank-tamper");
    let id = unique("bel-tamper");
    let mut b = mk_belief(&id, &bank, &unique("subj-tamper"));
    b.context = Some("secret context".into());
    store.create_belief(&b, "agent").unwrap();
    // Re-seal the context under a second subject, then destroy its DEK.
    let shred = unique("subj-shred-ctx");
    let ghost = kms
        .encrypt(&shred, b"ghost context")
        .unwrap()
        .to_storage_string();
    store
        .test_execute(&format!(
            "UPDATE beliefs SET context_cipher = '{ghost}' WHERE id = '{id}'"
        ))
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

#[test]
fn helpers_validate_and_map() {
    require_nonempty("x", "x").unwrap();
    assert_eq!(
        require_nonempty("", "bank_id").unwrap_err().code,
        ErrorCode::InvalidArgument
    );
    let a = unique_entry_id("b1", "t");
    let b = unique_entry_id("b1", "t");
    assert_ne!(a, b);
    assert!(a.starts_with("bce-b1-t"));
    map_belief_insert(Ok(1)).unwrap();
    assert_eq!(
        map_belief_insert(Err("UNIQUE constraint failed".into()))
            .unwrap_err()
            .code,
        ErrorCode::InvalidArgument
    );
    assert_eq!(
        map_belief_insert(Err("duplicate key value".into()))
            .unwrap_err()
            .code,
        ErrorCode::InvalidArgument
    );
    assert_eq!(
        map_belief_insert(Err("disk full".into())).unwrap_err().code,
        ErrorCode::Internal
    );
    validate_context_arg(None).unwrap();
    assert_eq!(
        validate_context_arg(Some("")).unwrap_err().code,
        ErrorCode::InvalidArgument
    );
    assert_eq!(
        validate_context_arg(Some(&"x".repeat(clio_types::CONTEXT_MAX_BYTES + 1)))
            .unwrap_err()
            .code,
        ErrorCode::OutOfRange
    );
}

#[test]
fn context_seal_open_and_stats_are_pii_safe() {
    let kms = LocalDevKms::new();
    let sealed = seal_belief_context(&kms, "subj", Some("oat milk")).unwrap();
    assert!(sealed.is_some());
    assert!(!sealed.clone().unwrap().contains("oat milk"));
    assert!(seal_belief_context(&kms, "subj", None).unwrap().is_none());
    assert_eq!(
        open_belief_context(&kms, sealed.as_deref()).as_deref(),
        Some("oat milk")
    );
    assert!(open_belief_context(&kms, None).is_none());
    assert!(open_belief_context(&kms, Some("")).is_none());
    assert!(open_belief_context(&kms, Some("not-an-envelope")).is_none());

    let stats = belief_context_stats(Some("oat milk"));
    assert!(!stats.to_string().contains("oat milk"));
    assert_eq!(stats["context_present"], serde_json::json!(true));
    assert_eq!(stats["context_len"], 8);
    let absent = belief_context_stats(None);
    assert_eq!(absent["context_present"], serde_json::json!(false));
    assert!(absent.get("context_hash").is_none());

    // Hash-only telemetry merges into the audit detail without raw values.
    let detail = belief_audit_detail("appended", 0.5, "user_stated", 2, Some(&stats));
    assert!(!detail.contains("oat milk"));
    assert!(detail.contains("\"context_len\":8"));
    assert!(detail.contains("\"context_hash\":"));
    let plain = belief_audit_detail("appended", 0.5, "user_stated", 2, None);
    assert!(!plain.contains("context_"));
}

#[test]
fn correction_detail_records_reason_prior_and_new_stats_only() {
    let detail = belief_context_correction_detail(Some("oat milk"), "soy", "fix");
    let value: serde_json::Value = serde_json::from_str(&detail).unwrap();
    assert_eq!(value["corrected"], serde_json::json!("context"));
    assert_eq!(value["reason"], serde_json::json!("fix"));
    assert_eq!(value["prior_context_present"], serde_json::json!(true));
    assert_eq!(value["prior_context_len"], 8);
    assert_eq!(value["context_present"], serde_json::json!(true));
    assert_eq!(value["context_len"], 3);
    assert_eq!(
        value["prior_context_hash"],
        crate::erase_hash::sha256_hex(crate::belief_store::BELIEF_CONTEXT_HASH_DOMAIN, "oat milk")
    );
    assert_eq!(
        value["context_hash"],
        crate::erase_hash::sha256_hex(crate::belief_store::BELIEF_CONTEXT_HASH_DOMAIN, "soy")
    );
    // Hash-only stats — no sealed cipher, no raw plaintext.
    assert!(
        value.get("prior_context_cipher").is_none(),
        "the prior sealed cipher must not be stored: {detail}"
    );
    assert!(!detail.contains("oat milk") && !detail.contains("soy"));
    let from_absent = belief_context_correction_detail(None, "soy", "r");
    let value: serde_json::Value = serde_json::from_str(&from_absent).unwrap();
    assert_eq!(value["prior_context_present"], serde_json::json!(false));
    assert!(value.get("prior_context_cipher").is_none());
    assert_eq!(value["prior_context_len"], 0);
}

/// Authorized correction on SQLite swaps only the sealed context: the value is
/// validated and re-sealed, the trajectory is untouched, and the audit row
/// carries the reason plus prior/new hash-only stats. The prior sealed value is
/// never retained, so the prior context is unrecoverable by design.
#[test]
fn sqlite_belief_context_correction_changes_only_context() {
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    let store = SqliteStore::open_in_memory(Arc::clone(&kms)).unwrap();
    let bank = unique("bank-bcorrect");
    let id = unique("bel-bcorrect");
    let mut b = mk_belief(&id, &bank, &unique("subj-bcorrect"));
    b.context = Some("original context".into());
    store.create_belief(&b, "agent").unwrap();

    store
        .correct_belief_context(&bank, &id, "corrected context", "operator fix", "agent")
        .unwrap();
    let got = store.get_belief(&bank, &id).unwrap().expect("belief");
    assert_eq!(got.context.as_deref(), Some("corrected context"));
    // Values/confidences evolve via appends only: the trajectory is untouched.
    assert_eq!(got.confidence_history.len(), 1);
    assert_eq!(got.confidence_history[0], b.confidence_history[0]);

    let (operation, epistemic_kind, confidence, detail): (String, String, Option<f64>, String) =
        store
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT operation, epistemic_kind, confidence, detail_json FROM audit_events
                     WHERE operation = 'correct' AND item_id = ?1
                     ORDER BY created_at, id",
                    [id.as_str()],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .am("audit query")
            })
            .unwrap();
    assert_eq!(operation, "correct");
    assert_eq!(epistemic_kind, "belief");
    // A correction observes nothing: the row carries no confidence.
    assert!(
        confidence.is_none(),
        "the correction row must not fabricate one"
    );
    let value: serde_json::Value = serde_json::from_str(&detail).unwrap();
    assert_eq!(value["corrected"], serde_json::json!("context"));
    assert_eq!(value["reason"], serde_json::json!("operator fix"));
    assert_eq!(value["prior_context_present"], serde_json::json!(true));
    assert_eq!(value["prior_context_len"], "original context".len());
    assert_eq!(
        value["prior_context_hash"],
        crate::erase_hash::sha256_hex(
            crate::belief_store::BELIEF_CONTEXT_HASH_DOMAIN,
            "original context"
        )
    );
    assert_eq!(value["context_len"], "corrected context".len());
    // Raw plaintext never enters telemetry, and the sealed prior cipher is not
    // retained either: the prior context is unrecoverable by design.
    assert!(!detail.contains("original context"));
    assert!(!detail.contains("corrected context"));
    assert!(
        value.get("prior_context_cipher").is_none(),
        "the sealed prior context must not be stored in the audit row: {detail}"
    );
}

/// Fail-closed inputs for the correction: unknown belief, oversized or empty
/// context, blank reason, and blank actor all reject before any write, and a
/// correction from an absent prior context records that absence.
#[test]
fn sqlite_belief_context_correction_fails_closed() {
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    let store = SqliteStore::open_in_memory(Arc::clone(&kms)).unwrap();
    let bank = unique("bank-bcfail");
    let id = unique("bel-bcfail");
    let mut b = mk_belief(&id, &bank, &unique("subj-bcfail"));
    b.context = Some("kept".into());
    store.create_belief(&b, "agent").unwrap();

    let oversized = "x".repeat(clio_types::CONTEXT_MAX_BYTES + 1);
    // Unknown belief fails with the shared belief not-found usage error.
    let err = store
        .correct_belief_context(&bank, &unique("bel-missing"), "v", "r", "agent")
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    for (new_context, reason, actor, code) in [
        (oversized.as_str(), "r", "agent", ErrorCode::OutOfRange),
        ("", "r", "agent", ErrorCode::InvalidArgument),
        ("valid", "", "agent", ErrorCode::InvalidArgument),
        ("valid", "r", "", ErrorCode::InvalidArgument),
    ] {
        let err = store
            .correct_belief_context(&bank, &id, new_context, reason, actor)
            .unwrap_err();
        assert_eq!(err.code, code, "{code:?}: {err}");
    }
    let got = store.get_belief(&bank, &id).unwrap().expect("belief");
    assert_eq!(got.context.as_deref(), Some("kept"));
    assert_eq!(got.confidence_history.len(), 1);

    // Correcting from an absent prior context records `prior_context_present`
    // as false and carries no prior cipher.
    let bare = unique("bel-bcfail-absent");
    let mut absent = mk_belief(&bare, &bank, &unique("subj-bcfail"));
    absent.proposition = "User Is Likely Vegan".into();
    absent.context = None;
    store.create_belief(&absent, "agent").unwrap();
    store
        .correct_belief_context(&bank, &bare, "new context", "operator fix", "agent")
        .unwrap();
    let detail: String = store
        .with_conn(|conn| {
            conn.query_row(
                "SELECT detail_json FROM audit_events
                 WHERE operation = 'correct' AND item_id = ?1",
                [bare.as_str()],
                |r| r.get(0),
            )
            .am("audit detail read")
        })
        .unwrap();
    let value: serde_json::Value = serde_json::from_str(&detail).unwrap();
    assert_eq!(value["prior_context_present"], serde_json::json!(false));
    assert!(value.get("prior_context_cipher").is_none());
    assert_eq!(value["context_present"], serde_json::json!(true));
}

#[path = "sqlite_belief_context_tests_stamp_tests.rs"]
mod belief_context_stamp_tests;
