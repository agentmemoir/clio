// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for tests of the SQLite persona repository's
//! optional `context`: write/read roundtrip, preserve-on-omit,
//! replace-on-supply, JSON omission, unreadable-cipher tolerance, and the
//! audited context lifecycle.
//!
//! ## Owns
//! - SQLite persona context persistence, preservation, and audit assertions
//!
//! ## Does not own
//! - Postgres persona context parity (see `pg_persona_tests`)
//! - Persona domain rules such as admission and budget (`clio-persona`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;
use crate::dek::DekProvider;
use clio_types::ErrorCode;

fn stable_entry(bank: &str, key: &str, value: &str, context: Option<&str>) -> PersonaStableEntry {
    PersonaStableEntry {
        bank_id: bank.into(),
        key: key.into(),
        value: value.into(),
        confidence: Some(0.5),
        source: "explicit".into(),
        source_ref: None,
        context: context.map(str::to_owned),
        admission_score: 0.5,
        as_of: "t".into(),
        valid_until: None,
    }
}

fn pref_entry(bank: &str, key: &str, value: f64, context: Option<&str>) -> PersonaPreference {
    PersonaPreference {
        bank_id: bank.into(),
        key: key.into(),
        value,
        trend: value,
        confidence: None,
        source: "inferred".into(),
        source_ref: None,
        context: context.map(str::to_owned),
        admission_score: 0.5,
        as_of: "t".into(),
    }
}

/// Stable entries: a version written with context surfaces it on the
/// document and in history; an ordinary update that omits context
/// preserves the prior version's context a key never written with
/// context omits the JSON key; an invalid context fails closed before any
/// write.
#[test]
fn sqlite_stable_context_write_read_and_reject() {
    let kms = std::sync::Arc::new(crate::dek::LocalDevKms::new());
    let store = SqliteStore::open_in_memory(kms).expect("sqlite");
    let bank = format!("bank-sq-ctx-{}", std::process::id());
    PersonaStore::put_stable_entry(
        &store,
        &stable_entry(&bank, "style", "concise", Some("prefers short replies")),
        "agent",
    )
    .unwrap();
    let doc = PersonaStore::get_persona_document(&store, &bank).unwrap();
    assert_eq!(
        doc.stable[0].context.as_deref(),
        Some("prefers short replies")
    );
    let history = PersonaStore::stable_history(&store, &bank, "style").unwrap();
    assert_eq!(history[0].context.as_deref(), Some("prefers short replies"));

    // Ordinary update without context: the prior open version's context is
    // carried into the new version; the closed prior
    // version keeps its own context in history.
    PersonaStore::put_stable_entry(
        &store,
        &stable_entry(&bank, "style", "detailed", None),
        "agent",
    )
    .unwrap();
    let doc = PersonaStore::get_persona_document(&store, &bank).unwrap();
    assert_eq!(doc.stable[0].value, "detailed");
    assert_eq!(
        doc.stable[0].context.as_deref(),
        Some("prefers short replies")
    );
    let history = PersonaStore::stable_history(&store, &bank, "style").unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].context.as_deref(), Some("prefers short replies"));
    assert_eq!(history[1].context.as_deref(), Some("prefers short replies"));

    // A key never written with context omits the JSON key.
    PersonaStore::put_stable_entry(&store, &stable_entry(&bank, "plain", "v", None), "agent")
        .unwrap();
    let doc = PersonaStore::get_persona_document(&store, &bank).unwrap();
    let plain = doc.stable.iter().find(|e| e.key == "plain").unwrap();
    assert!(plain.context.is_none());
    assert!(
        !serde_json::to_string(plain)
            .unwrap()
            .contains("\"context\""),
        "records without context omit the key"
    );

    // Invalid context fails closed at the repository boundary.
    let oversized = "a".repeat(4097);
    for bad in ["", oversized.as_str()] {
        let err = PersonaStore::put_stable_entry(
            &store,
            &stable_entry(&bank, "bad", "v", Some(bad)),
            "agent",
        )
        .unwrap_err();
        assert!(
            err.code == ErrorCode::InvalidArgument || err.code == ErrorCode::OutOfRange,
            "{err}"
        );
    }
    let doc = PersonaStore::get_persona_document(&store, &bank).unwrap();
    assert!(!doc.stable.iter().any(|e| e.key == "bad"));
}

/// Preference context: supplied context replaces the row's stored cipher;
/// an omitted context preserves it (ordinary updates never clear it);
/// invalid contexts fail closed before any write.
#[test]
fn sqlite_preference_context_replace_and_preserve() {
    let kms = std::sync::Arc::new(crate::dek::LocalDevKms::new());
    let store = SqliteStore::open_in_memory(kms).expect("sqlite");
    let bank = format!("bank-sq-pctx-{}", std::process::id());
    PersonaStore::put_preference_state(
        &store,
        &pref_entry(&bank, "brevity_affinity", 0.5, Some("week one note")),
        "agent",
    )
    .unwrap();
    assert_eq!(
        PersonaStore::get_preference(&store, &bank, "brevity_affinity")
            .unwrap()
            .unwrap()
            .context
            .as_deref(),
        Some("week one note")
    );
    // Omitted context: preserved.
    PersonaStore::put_preference_state(
        &store,
        &pref_entry(&bank, "brevity_affinity", 0.7, None),
        "agent",
    )
    .unwrap();
    assert_eq!(
        PersonaStore::get_preference(&store, &bank, "brevity_affinity")
            .unwrap()
            .unwrap()
            .context
            .as_deref(),
        Some("week one note")
    );
    // New context: replaced (also in the assembled document).
    PersonaStore::put_preference_state(
        &store,
        &pref_entry(&bank, "brevity_affinity", 0.8, Some("week two note")),
        "agent",
    )
    .unwrap();
    let doc = PersonaStore::get_persona_document(&store, &bank).unwrap();
    assert_eq!(doc.preferences[0].context.as_deref(), Some("week two note"));
    // Invalid context fails closed; nothing is written.
    let err = PersonaStore::put_preference_state(
        &store,
        &pref_entry(&bank, "bad_pref", 0.5, Some("")),
        "agent",
    )
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert!(
        PersonaStore::get_preference(&store, &bank, "bad_pref")
            .unwrap()
            .is_none()
    );
}

/// Unreadable context never breaks a read: a corrupted `context_cipher` on
/// a stable row opens to `None` (document + history), and a shredded DEK
/// leaves `get_preference` working with `context: None` (numeric columns
/// need no DEK).
#[test]
fn sqlite_unreadable_context_reads_still_succeed() {
    let kms = std::sync::Arc::new(crate::dek::LocalDevKms::new());
    let store = SqliteStore::open_in_memory(kms.clone()).expect("sqlite");
    let bank = format!("bank-sq-shred-{}", std::process::id());
    PersonaStore::put_stable_entry(
        &store,
        &stable_entry(&bank, "style", "concise", Some("note")),
        "agent",
    )
    .unwrap();
    PersonaStore::put_preference_state(
        &store,
        &pref_entry(&bank, "brevity_affinity", 0.5, Some("note")),
        "agent",
    )
    .unwrap();
    // Corrupt only the stable row's context cipher.
    store
        .test_execute(&format!(
            "UPDATE persona_stable_entries SET context_cipher = 'not-json'
                 WHERE bank_id = '{bank}'"
        ))
        .unwrap();
    let doc = PersonaStore::get_persona_document(&store, &bank).unwrap();
    assert_eq!(doc.stable[0].value, "concise");
    assert!(doc.stable[0].context.is_none());
    let history = PersonaStore::stable_history(&store, &bank, "style").unwrap();
    assert!(history[0].context.is_none());
    // Crypto-shred the bank's DEK: the preference read still succeeds.
    kms.destroy_dek(&bank).expect("shred dek");
    let pref = PersonaStore::get_preference(&store, &bank, "brevity_affinity")
        .unwrap()
        .expect("row survives erasure");
    assert!((pref.value - 0.5).abs() < 1e-12);
    assert!(pref.context.is_none());
}

/// All persona audit rows for one bank as `(operation, item_id, detail_json)`,
/// oldest first.
fn persona_audit_rows(store: &SqliteStore, bank: &str) -> Vec<(String, String, String)> {
    store
        .with_conn(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT operation, item_id, detail_json FROM audit_events
                      WHERE bank_id = ?1 AND operation LIKE 'persona_%'
                      ORDER BY created_at, id",
                )
                .expect("audit prepare");
            let rows = stmt
                .query_map(rusqlite::params![bank], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?))
                })
                .expect("audit query");
            rows.collect::<Result<Vec<_>, _>>().map_err(|e| {
                clio_types::AmError::new(clio_types::ErrorCode::Internal, e.to_string())
            })
        })
        .expect("audit rows")
}

/// Context replacement is the explicit audited correction:
///  every persona write emits an `audit_events` row whose detail
/// records presence/length/hash for the prior and effective context —
/// never the raw text and never a sealed copy of it, so erasure cannot leave
/// a decryptable derived copy in the append-only audit table. Stable prior
/// contexts stay reconstructable through the closed history row until the
/// erase path clears that column.
#[test]
fn sqlite_persona_context_replacement_writes_audit() {
    let kms = std::sync::Arc::new(crate::dek::LocalDevKms::new());
    let store = SqliteStore::open_in_memory(kms.clone()).expect("sqlite");
    let bank = format!("bank-sq-audit-{}", std::process::id());
    PersonaStore::put_stable_entry(
        &store,
        &stable_entry(&bank, "style", "concise", Some("prior stable note")),
        "agent",
    )
    .unwrap();
    PersonaStore::put_stable_entry(
        &store,
        &stable_entry(&bank, "style", "detailed", Some("new stable note")),
        "agent",
    )
    .unwrap();
    PersonaStore::put_preference_state(
        &store,
        &pref_entry(&bank, "brevity_affinity", 0.5, Some("week one note")),
        "agent",
    )
    .unwrap();
    PersonaStore::put_preference_state(
        &store,
        &pref_entry(&bank, "brevity_affinity", 0.8, Some("week two note")),
        "agent",
    )
    .unwrap();

    let rows = persona_audit_rows(&store, &bank);
    assert!(rows.len() >= 4, "one audit row per persona write: {rows:?}");

    // The audit detail carries hash-only stats, never the raw context.
    for raw in [
        "prior stable note",
        "new stable note",
        "week one note",
        "week two note",
    ] {
        for (_, _, d) in &rows {
            assert!(!d.contains(raw), "raw context leaked into audit: {d}");
        }
    }
    // Stable write stats: prior + effective context recorded for change
    // detection; no ciphertext in the stable audit detail (the closed
    // history row retains the prior sealed value). The create-with-context
    // write records `set`; the explicit replacement records `replaced`.
    let stable_rows: Vec<&(String, String, String)> = rows
        .iter()
        .filter(|(op, _, _)| op == "persona_put_stable")
        .collect();
    let first: serde_json::Value = serde_json::from_str(stable_rows[0].2.as_str()).unwrap();
    assert_eq!(first["context_action"], "set");
    assert_eq!(first["context_prior_present"], serde_json::json!(false));
    assert_eq!(first["context_present"], serde_json::json!(true));
    let stable_row = stable_rows
        .iter()
        .next_back()
        .expect("at least two stable rows");
    let detail: serde_json::Value = serde_json::from_str(stable_row.2.as_str()).unwrap();
    assert_eq!(detail["context_action"], "replaced");
    assert_eq!(detail["context_prior_present"], serde_json::json!(true));
    assert_eq!(detail["context_present"], serde_json::json!(true));
    assert!(!detail.to_string().contains("cipher"));
    // The prior context is reconstructable through the closed version row.
    let history = PersonaStore::stable_history(&store, &bank, "style").unwrap();
    assert_eq!(history[0].context.as_deref(), Some("prior stable note"));

    // Preference replacement: the audit detail keeps hash-only stats and no
    // sealed copy of the replaced value, so erasing the subject cannot leave a
    // decryptable derived copy behind. The live row holds only the new
    // value.
    let replace_row = rows
        .iter()
        .rev()
        .find(|(op, _, d)| {
            op == "persona_put_preference"
                && serde_json::from_str::<serde_json::Value>(d)
                    .map(|v| v["context_action"] == "replaced")
                    .unwrap_or(false)
        })
        .expect("replacement audit row");
    let detail: serde_json::Value = serde_json::from_str(replace_row.2.as_str()).unwrap();
    assert_eq!(detail["context_prior_present"], serde_json::json!(true));
    // Assertion corrected: `"week one note"` is 13 bytes (12 was inconsistent).
    assert_eq!(detail["context_prior_len"], serde_json::json!(13));
    assert!(
        detail.get("context_prior_cipher").is_none(),
        "audit detail must not retain a sealed copy: {detail}"
    );
    assert!(
        !replace_row.2.contains("cipher"),
        "audit detail must stay cipher-free: {}",
        replace_row.2
    );
    let stored = PersonaStore::get_preference(&store, &bank, "brevity_affinity")
        .unwrap()
        .expect("stored preference");
    assert_eq!(stored.context.as_deref(), Some("week two note"));
}
