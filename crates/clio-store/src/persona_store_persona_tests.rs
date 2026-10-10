// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for tests of the persona-document repository
//! (shared contract suite plus SQLite and Postgres backend runs).
//!
//! ## Owns
//! - stable put + invalidation + retained history
//! - cross-bank isolation, DEK-at-rest inspection, preference score retention
//!
//! ## Does not own
//! - Tool-handler tests (`clio-persona`)
//!
//! ## Boundary
//!
//! Repository contract changes should normally update tests here.

use crate::dek::{DekProvider, LocalDevKms};
use crate::persona_store::PersonaStore;
use clio_types::{ErrorCode, PersonaPreference, PersonaStableEntry};
use std::sync::Arc;

use crate::test_unique::unique;

pub(crate) fn stable(
    bank: &str,
    key: &str,
    value: &str,
    score: f64,
    as_of: &str,
) -> PersonaStableEntry {
    PersonaStableEntry {
        context: None,
        bank_id: bank.into(),
        key: key.into(),
        value: value.into(),
        confidence: Some(0.8),
        source: "explicit".into(),
        source_ref: Some("turn-1".into()),
        admission_score: score,
        as_of: as_of.into(),
        valid_until: None,
    }
}

pub(crate) fn pref(bank: &str, key: &str, value: f64, trend: f64, score: f64) -> PersonaPreference {
    PersonaPreference {
        context: None,
        bank_id: bank.into(),
        key: key.into(),
        value,
        trend,
        confidence: Some(0.7),
        source: "inferred".into(),
        source_ref: Some("turn-2".into()),
        admission_score: score,
        as_of: "2026-09-10".into(),
    }
}

/// Contract assertions any [`PersonaStore`] + KMS pair must satisfy.
pub fn run_persona_suite(store: &dyn PersonaStore, kms: &dyn DekProvider) {
    let bank = unique("bank-persona");
    let other = unique("bank-other");
    let key = "communication_style";

    // Put then read back.
    store
        .put_stable_entry(&stable(&bank, key, "concise", 0.9, "2026-09-10"), "agent")
        .expect("put stable");
    let doc = store.get_persona_document(&bank).expect("doc");
    assert_eq!(doc.stable.len(), 1);
    let entry = &doc.stable[0];
    assert_eq!(entry.key, key);
    assert_eq!(entry.value, "concise");
    assert!((entry.admission_score - 0.9).abs() < 1e-9);
    assert_eq!(entry.source, "explicit");

    // Second put supersedes; prior version stays in history.
    store
        .put_stable_entry(&stable(&bank, key, "detailed", 0.7, "2026-09-12"), "agent")
        .expect("put stable v2");
    let doc = store.get_persona_document(&bank).expect("doc");
    assert_eq!(doc.stable.len(), 1);
    assert_eq!(doc.stable[0].value, "detailed");
    let history = store.stable_history(&bank, key).expect("history");
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].value, "concise");
    assert_eq!(history[0].valid_until.as_deref(), Some("2026-09-12"));
    assert_eq!(history[1].value, "detailed");
    assert!(history[1].valid_until.is_none());

    // An ordinary stable update that omits context preserves the prior
    // version's context (the store carries it into the new version); the
    // prior value stays reconstructable through the retained history.
    let mut tone_v1 = stable(&bank, "tone", "warm", 0.8, "2026-09-14");
    tone_v1.context = Some("user prefers a warm tone".to_owned());
    store.put_stable_entry(&tone_v1, "agent").expect("tone v1");
    store
        .put_stable_entry(&stable(&bank, "tone", "cool", 0.8, "2026-09-15"), "agent")
        .expect("tone v2");
    let doc = store.get_persona_document(&bank).expect("doc");
    let tone = doc.stable.iter().find(|e| e.key == "tone").expect("tone");
    assert_eq!(tone.value, "cool");
    assert_eq!(tone.context.as_deref(), Some("user prefers a warm tone"));
    let hist = store.stable_history(&bank, "tone").expect("history");
    assert_eq!(hist.len(), 2);
    assert_eq!(hist[0].context.as_deref(), Some("user prefers a warm tone"));
    assert_eq!(hist[1].context.as_deref(), Some("user prefers a warm tone"));

    // Bank isolation — another bank sees nothing.
    let other_doc = store.get_persona_document(&other).unwrap();
    assert!(other_doc.stable.is_empty(), "{:?}", other_doc.stable);
    let other_history = store.stable_history(&other, key).unwrap();
    assert!(other_history.is_empty(), "{other_history:?}");
    assert!(
        store
            .inspect_stable_ciphertext(&other, key)
            .unwrap()
            .is_none()
    );

    // Value is encrypted at rest under the bank DEK (subject id = bank id).
    let raw = store
        .inspect_stable_ciphertext(&bank, key)
        .unwrap()
        .expect("cipher");
    assert!(!raw.contains("detailed"));
    assert!(raw.contains("ciphertext_b64"));
    // DEK ref persisted for the bank subject.
    let dek_ref = kms.ensure_dek(&bank).expect("dek");
    assert!(!dek_ref.is_empty(), "{dek_ref:?}");

    // Preference create stamps the score; later upserts retain it.
    // Targeted preference read: exact row without touching stable entries.
    run_preference_suite(store);

    // Validation rejects at the repository boundary.
    assert_eq!(
        store
            .put_stable_entry(&stable(&bank, "k", "v", 1.5, "t"), "agent")
            .unwrap_err()
            .code,
        ErrorCode::OutOfRange
    );
    assert_eq!(
        store
            .put_stable_entry(&stable(&bank, "", "v", 0.5, "t"), "agent")
            .unwrap_err()
            .code,
        ErrorCode::InvalidArgument
    );
    assert_eq!(
        store
            .put_preference_state(&pref(&bank, "k", f64::NAN, 0.0, 0.5), "agent")
            .unwrap_err()
            .code,
        ErrorCode::OutOfRange
    );
    assert_eq!(
        store
            .put_stable_entry(&stable(&bank, key, "v", 0.5, "t"), "")
            .unwrap_err()
            .code,
        ErrorCode::InvalidArgument
    );
}

/// Preference contract: create/upsert score retention plus the targeted
/// single-row read (`get_preference`), isolated from the stable-entry suite.
fn run_preference_suite(store: &dyn PersonaStore) {
    let bank = unique("bank-persona-pref");
    let other = unique("bank-other-pref");
    store
        .put_preference_state(&pref(&bank, "brevity_affinity", 0.5, 0.5, 0.6), "agent")
        .expect("pref create");
    store
        .put_preference_state(&pref(&bank, "brevity_affinity", 0.75, 0.62, 0.6), "agent")
        .expect("pref observe");
    let doc = store.get_persona_document(&bank).expect("doc");
    assert_eq!(doc.preferences.len(), 1);
    let p = &doc.preferences[0];
    assert!((p.value - 0.75).abs() < 1e-9);
    assert!((p.trend - 0.62).abs() < 1e-9);
    assert!(
        (p.admission_score - 0.6).abs() < 1e-9,
        "create score retained"
    );
    let got = store
        .get_preference(&bank, "brevity_affinity")
        .expect("pref read");
    assert_eq!(
        got.as_ref().map(|p| p.key.as_str()),
        Some("brevity_affinity")
    );
    assert!((got.as_ref().unwrap().value - 0.75).abs() < 1e-9);
    // Missing key and cross-bank reads fail closed with `None`.
    assert!(
        store
            .get_preference(&bank, "no_such_key")
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .get_preference(&other, "brevity_affinity")
            .unwrap()
            .is_none()
    );
}

#[test]
fn sqlite_persona_suite() {
    let kms = Arc::new(LocalDevKms::new());
    let store = crate::sqlite::SqliteStore::open_in_memory(kms.clone()).expect("sqlite");
    run_persona_suite(&store, kms.as_ref());
}

#[test]
fn postgres_persona_suite() {
    let _guard = crate::lock_pg_test();
    let url = crate::db_url::postgres_url_from_env();
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    let store = crate::postgres::PostgresStore::open(url.as_str(), kms.clone()).expect("pg");
    run_persona_suite(&store, kms.as_ref());
}
