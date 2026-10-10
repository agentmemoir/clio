// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for Postgres-specific persona repository tests.
//!
//! ## Owns
//! - dual-backend parity sample (same puts → same document)
//! - Optional `context` dual-backend parity (surface / preserve / replace /
//!   JSON omission / unreadable-cipher tolerance)
//! - Schema inspection: `source` column, no `source_type`
//!
//! ## Boundary
//!
//! Postgres persona behavior changes should normally update tests here.

use crate::dek::{DekProvider, LocalDevKms};
use crate::persona_store::PersonaStore;
use crate::persona_store::persona_store_persona_tests::{pref, stable};
use clio_types::{PersonaPreference, PersonaStableEntry};
use std::sync::Arc;

/// Parity sample — the same writes produce the same document on both
/// backends.
#[test]
fn persona_document_parity_sqlite_vs_postgres() {
    let _guard = crate::lock_pg_test();
    let url = crate::db_url::postgres_url_from_env();
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    let pg = crate::postgres::PostgresStore::open(url.as_str(), kms.clone()).expect("pg");
    let lite = crate::sqlite::SqliteStore::open_in_memory(kms.clone()).expect("sqlite");

    let bank = format!(
        "bank-parity-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    );
    let expected_stable = stable(&bank, "communication_style", "concise", 0.9, "2026-09-10");
    let expected_pref = pref(&bank, "brevity_affinity", 0.5, 0.5, 0.6);
    for store in [&lite as &dyn PersonaStore, &pg] {
        store
            .put_stable_entry(&expected_stable, "agent")
            .expect("put stable");
        store
            .put_preference_state(&expected_pref, "agent")
            .expect("pref create");
    }
    let a = lite.get_persona_document(&bank).expect("sqlite doc");
    let b = pg.get_persona_document(&bank).expect("pg doc");
    // Values/trends equal modulo timestamps; scores present on both.
    assert_eq!(a.stable.len(), 1);
    assert_eq!(b.stable.len(), 1);
    assert_eq!(a.stable[0].key, b.stable[0].key);
    assert_eq!(a.stable[0].value, b.stable[0].value);
    assert!((a.stable[0].admission_score - b.stable[0].admission_score).abs() < 1e-12);
    assert_eq!(a.preferences.len(), 1);
    assert_eq!(b.preferences.len(), 1);
    assert!((a.preferences[0].value - b.preferences[0].value).abs() < 1e-12);
    assert!((a.preferences[0].admission_score - b.preferences[0].admission_score).abs() < 1e-12);
}

/// The persona provenance column is `source`; `source_type` never exists
/// on persona tables (belief provenance stays belief-scoped).
#[test]
fn persona_schema_uses_source_column() {
    let _guard = crate::lock_pg_test();
    let url = crate::db_url::postgres_url_from_env();
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    let pg = crate::postgres::PostgresStore::open(url.as_str(), kms).expect("pg");
    let cols = pg
        .with_client(|client| {
            client
                .query(
                    "SELECT table_name, column_name FROM information_schema.columns
                     WHERE table_schema = 'public'
                       AND table_name IN ('persona_stable_entries', 'persona_preferences')",
                    &[],
                )
                .map_err(|e| {
                    clio_types::AmError::new(clio_types::ErrorCode::Internal, e.to_string())
                })
        })
        .expect("columns");
    let names: Vec<(String, String)> = cols.into_iter().map(|r| (r.get(0), r.get(1))).collect();
    for table in ["persona_stable_entries", "persona_preferences"] {
        assert!(
            names.iter().any(|(t, c)| t == table && c == "source"),
            "{table} must have a `source` column"
        );
        assert!(
            !names.iter().any(|(t, c)| t == table && c == "source_type"),
            "{table} must not have a `source_type` column"
        );
        assert!(
            names
                .iter()
                .any(|(t, c)| t == table && c == "admission_score"),
            "{table} must have an admission_score column"
        );
    }
}

/// A supplied preference context records the write's `as_of` as its
/// effective time (`context_as_of`); an ordinary update preserves both the
/// context and that time.
#[test]
fn persona_preference_context_as_of_tracking() {
    let _guard = crate::lock_pg_test();
    let url = crate::db_url::postgres_url_from_env();
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    let pg = crate::postgres::PostgresStore::open(url.as_str(), kms).expect("pg");
    let bank = format!(
        "bank-ctx-asof-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    );
    let pref = |value: f64, context: Option<&str>, as_of: &str| PersonaPreference {
        bank_id: bank.clone(),
        key: "brevity_affinity".into(),
        value,
        trend: value,
        confidence: None,
        source: "inferred".into(),
        source_ref: None,
        context: context.map(str::to_owned),
        admission_score: 0.6,
        as_of: as_of.into(),
    };
    pg.put_preference_state(&pref(0.5, Some("week one note"), "2026-09-10"), "agent")
        .expect("pref create");
    pg.put_preference_state(&pref(0.75, None, "2026-09-20"), "agent")
        .expect("pref observe");
    let (has_cipher, context_as_of) = pg
        .with_client(|client| {
            client
                .query_one(
                    "SELECT context_cipher IS NOT NULL, context_as_of
                       FROM persona_preferences WHERE bank_id = $1 AND attr_key = $2",
                    &[&bank, &"brevity_affinity"],
                )
                .map(|r| (r.get::<_, bool>(0), r.get::<_, Option<String>>(1)))
                .map_err(|e| {
                    clio_types::AmError::new(clio_types::ErrorCode::Internal, e.to_string())
                })
        })
        .expect("preference row");
    assert!(has_cipher, "the supplied context is stored");
    assert_eq!(
        context_as_of.as_deref(),
        Some("2026-09-10"),
        "an ordinary update preserves the context's effective time"
    );
}

/// Corrupt the stable-entry context cipher on one backend so the read must
/// tolerate an unreadable body without failing.
fn corrupt_stable_context(
    backend: &str,
    lite: &crate::sqlite::SqliteStore,
    pg: &crate::postgres::PostgresStore,
    bank: &str,
) {
    match backend {
        "sqlite" => lite
            .test_execute(&format!(
                "UPDATE persona_stable_entries SET context_cipher = 'not-json'
                 WHERE bank_id = '{bank}'"
            ))
            .unwrap(),
        _ => pg
            .with_client(|client| {
                client
                    .execute(
                        "UPDATE persona_stable_entries SET context_cipher = 'not-json'
                         WHERE bank_id = $1",
                        &[&bank],
                    )
                    .map_err(|e| {
                        clio_types::AmError::new(clio_types::ErrorCode::Internal, e.to_string())
                    })
                    .map(|_| ())
            })
            .expect("corrupt pg context"),
    }
}

/// Optional `context` parity: the same writes produce the same surfaced
/// context on both backends — stable entries per version, preference
/// preserve-on-omit / replace-on-supply, JSON omission, unreadable-cipher
/// tolerance (corrupted cipher on stables, shredded DEK on preferences).
/// Preserve/replace/omission/unreadable lifecycle for one backend (shared by
/// the SQLite-vs-Postgres parity test).
fn exercise_persona_context_lifecycle(store: &dyn PersonaStore, bank: &str) {
    let with_ctx = |value: &str, context: Option<&str>| PersonaStableEntry {
        bank_id: bank.to_owned(),
        key: "communication_style".into(),
        value: value.into(),
        confidence: Some(0.8),
        source: "explicit".into(),
        source_ref: None,
        context: context.map(str::to_owned),
        admission_score: 0.9,
        as_of: "2026-09-10".into(),
        valid_until: None,
    };
    let pref_with_ctx = |value: f64, context: Option<&str>| PersonaPreference {
        bank_id: bank.to_owned(),
        key: "brevity_affinity".into(),
        value,
        trend: value,
        confidence: Some(0.7),
        source: "inferred".into(),
        source_ref: None,
        context: context.map(str::to_owned),
        admission_score: 0.6,
        as_of: "2026-09-10".into(),
    };
    // Stable: v1 with context, v2 without (the omitted context is
    // preserved into v2; the closed v1 keeps its own).
    store
        .put_stable_entry(&with_ctx("concise", Some("prefers short replies")), "agent")
        .expect("put stable v1");
    store
        .put_stable_entry(&with_ctx("detailed", None), "agent")
        .expect("put stable v2");
    let doc = store.get_persona_document(bank).expect("doc");
    assert_eq!(doc.stable[0].value, "detailed");
    assert_eq!(
        doc.stable[0].context.as_deref(),
        Some("prefers short replies")
    );
    let history = store.stable_history(bank, "communication_style").unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].context.as_deref(), Some("prefers short replies"));
    assert_eq!(history[1].context.as_deref(), Some("prefers short replies"));

    // A key never written with context omits the JSON key.
    let mut plain = with_ctx("neutral", None);
    plain.key = "plain_style".into();
    store
        .put_stable_entry(&plain, "agent")
        .expect("put stable plain");
    let doc = store.get_persona_document(bank).expect("doc");
    let plain = doc.stable.iter().find(|e| e.key == "plain_style").unwrap();
    assert!(plain.context.is_none());
    assert!(
        !serde_json::to_string(plain)
            .unwrap()
            .contains("\"context\""),
        "records without context omit the key"
    );

    // Preference: supplied context replaces; omitted preserves.
    store
        .put_preference_state(&pref_with_ctx(0.5, Some("week one note")), "agent")
        .expect("pref create");
    store
        .put_preference_state(&pref_with_ctx(0.75, None), "agent")
        .expect("pref observe");
    let pref = store
        .get_preference(bank, "brevity_affinity")
        .unwrap()
        .unwrap();
    assert!((pref.value - 0.75).abs() < 1e-12);
    assert_eq!(pref.context.as_deref(), Some("week one note"));
    store
        .put_preference_state(&pref_with_ctx(0.8, Some("week two note")), "agent")
        .expect("pref replace");
    let pref = store
        .get_preference(bank, "brevity_affinity")
        .unwrap()
        .unwrap();
    assert_eq!(pref.context.as_deref(), Some("week two note"));
}

#[test]
fn persona_context_parity_sqlite_vs_postgres() {
    let _guard = crate::lock_pg_test();
    let url = crate::db_url::postgres_url_from_env();
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    let pg = crate::postgres::PostgresStore::open(url.as_str(), kms.clone()).expect("pg");
    let lite = crate::sqlite::SqliteStore::open_in_memory(kms.clone()).expect("sqlite");

    let bank = format!(
        "bank-ctx-parity-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    );
    for (backend, store) in [
        ("sqlite", &lite as &dyn PersonaStore),
        ("postgres", &pg as &dyn PersonaStore),
    ] {
        exercise_persona_context_lifecycle(store, &bank);

        // Unreadable stable context (corrupted cipher) opens to `None`.
        corrupt_stable_context(backend, &lite, &pg, &bank);
        let doc = store
            .get_persona_document(&bank)
            .expect("doc after corrupt");
        assert!(doc.stable[0].context.is_none());
        let history = store.stable_history(&bank, "communication_style").unwrap();
        assert!(history.iter().all(|e| e.context.is_none()));
    }

    // Shredded DEK: preference reads still succeed with `context: None`.
    kms.destroy_dek(&bank).expect("shred dek");
    for store in [&lite as &dyn PersonaStore, &pg] {
        let pref = store
            .get_preference(&bank, "brevity_affinity")
            .unwrap()
            .unwrap();
        assert!(pref.context.is_none(), "shredded context opens to None");
    }
}
