// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for tests of the SQLite persona repository:
//! static-path execution, storage failure modes, and driver-error mapping.
//!
//! ## Owns
//! - sqlite_persona unit tests (failure-mode and driver-error coverage)
//! - Optional `context` coverage: write/read roundtrip, preserve-on-omit,
//!   replace-on-supply, JSON omission, and unreadable-cipher tolerance
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;
mod tests {
    use super::*;

    #[test]
    fn sqlite_persona_paths_execute_statically() {
        let kms = std::sync::Arc::new(crate::dek::LocalDevKms::new());
        let store = SqliteStore::open_in_memory(kms).expect("sqlite");
        let bank = format!("bank-sqlite-persona-{}", std::process::id());
        let entry = PersonaStableEntry {
            bank_id: bank.clone(),
            key: "style".into(),
            value: "concise".into(),
            confidence: Some(0.5),
            source: "explicit".into(),
            source_ref: None,
            context: None,
            admission_score: 0.5,
            as_of: "t".into(),
            valid_until: None,
        };
        PersonaStore::put_stable_entry(&store, &entry, "agent").unwrap();
        let history = PersonaStore::stable_history(&store, &bank, "style").unwrap();
        assert_eq!(history.len(), 1);
        let doc = PersonaStore::get_persona_document(&store, &bank).unwrap();
        assert_eq!(doc.stable[0].value, "concise");
        assert!(
            PersonaStore::inspect_stable_ciphertext(&store, &bank, "style")
                .unwrap()
                .is_some()
        );
        let pref = PersonaPreference {
            bank_id: bank.clone(),
            key: "affinity".into(),
            value: 0.5,
            trend: 0.5,
            confidence: None,
            source: "inferred".into(),
            source_ref: None,
            context: None,
            admission_score: 0.5,
            as_of: "t".into(),
        };
        PersonaStore::put_preference_state(&store, &pref, "agent").unwrap();
    }
}

mod failure_mode_tests {
    use super::*;
    use crate::migrate::now_iso;
    use clio_types::ErrorCode;

    /// Failure-mode coverage: persona reads/writes fail closed with structured
    /// errors when the storage surface breaks (table dropped underneath them).
    #[test]
    fn persona_fails_closed_on_storage_failure() {
        let kms = std::sync::Arc::new(crate::dek::LocalDevKms::new());
        let store = SqliteStore::open_in_memory(kms).expect("sqlite");
        let bank = format!("bank-fail-{}", std::process::id());
        let entry = PersonaStableEntry {
            bank_id: bank.clone(),
            key: "style".into(),
            value: "concise".into(),
            confidence: Some(0.5),
            source: "explicit".into(),
            source_ref: None,
            context: None,
            admission_score: 0.5,
            as_of: "t".into(),
            valid_until: None,
        };
        PersonaStore::put_stable_entry(&store, &entry, "agent").unwrap();
        // Point-in-time arguments are validated at the boundary.
        assert_eq!(
            PersonaStore::stable_history(&store, &bank, "")
                .unwrap_err()
                .code,
            ErrorCode::InvalidArgument
        );
        // Drive failure: the persona tables disappear under the store.
        store
            .test_execute("DROP TABLE persona_preferences")
            .unwrap();
        store
            .test_execute("DROP TABLE persona_stable_entries")
            .unwrap();
        assert_eq!(
            PersonaStore::put_stable_entry(&store, &entry, "agent")
                .unwrap_err()
                .code,
            ErrorCode::Internal
        );
        assert_eq!(
            PersonaStore::put_preference_state(
                &store,
                &PersonaPreference {
                    bank_id: bank.clone(),
                    key: "affinity".into(),
                    value: 0.5,
                    trend: 0.5,
                    confidence: None,
                    source: "inferred".into(),
                    source_ref: None,
                    context: None,
                    admission_score: 0.5,
                    as_of: now_iso(),
                },
                "agent",
            )
            .unwrap_err()
            .code,
            ErrorCode::Internal
        );
        assert_eq!(
            PersonaStore::get_persona_document(&store, &bank)
                .unwrap_err()
                .code,
            ErrorCode::Internal
        );
        assert_eq!(
            PersonaStore::stable_history(&store, &bank, "style")
                .unwrap_err()
                .code,
            ErrorCode::Internal
        );
        assert_eq!(
            PersonaStore::inspect_stable_ciphertext(&store, &bank, "style")
                .unwrap_err()
                .code,
            ErrorCode::Internal
        );
    }
}

mod driver_error_tests {
    use super::*;
    use clio_types::ErrorCode;

    /// Driver-error propagation: row-level decode failures and insert
    /// failures surface as structured errors (never partial documents).
    #[test]
    fn persona_maps_driver_errors() {
        let kms = std::sync::Arc::new(crate::dek::LocalDevKms::new());
        let store = SqliteStore::open_in_memory(kms).expect("sqlite");
        let bank = format!("bank-driver-{}", std::process::id());
        let ts = crate::migrate::now_iso();
        // A BLOB in a text column breaks row decoding on every persona read.
        store
            .test_execute(&format!(
                "INSERT INTO persona_stable_entries (
                     id, bank_id, attr_key, value_cipher, confidence, source, source_ref,
                     admission_score, valid_from, tx_from, created_at, updated_at
                 ) VALUES ('dr-{ts}', '{bank}', 'style', X'00FF', NULL, 'explicit', NULL, 0.5, 't', '{ts}', '{ts}', '{ts}')"
            ))
            .unwrap();
        store
            .test_execute(&format!(
                "INSERT INTO persona_preferences (
                     id, bank_id, attr_key, value, trend, confidence, source, source_ref,
                     admission_score, as_of, created_at, updated_at
                 ) VALUES ('dp-{ts}', '{bank}', 'affinity', X'00', 0, NULL, 'inferred', NULL, 0.5, '{ts}', '{ts}', '{ts}')"
            ))
            .unwrap();
        let err = PersonaStore::get_persona_document(&store, &bank).unwrap_err();
        assert_eq!(err.code, ErrorCode::Internal, "{err}");
        let err = PersonaStore::stable_history(&store, &bank, "style").unwrap_err();
        assert_eq!(err.code, ErrorCode::Internal, "{err}");
        // Insert failure: a BEFORE INSERT trigger aborts inside the driver,
        // propagating through the insert error mapping.
        store
            .test_execute(
                "CREATE TRIGGER fail_pse BEFORE INSERT ON persona_stable_entries
                 BEGIN SELECT RAISE(ABORT, 'forced insert failure'); END",
            )
            .unwrap();
        let entry = PersonaStableEntry {
            bank_id: bank.clone(),
            key: "style".into(),
            value: "v".into(),
            confidence: None,
            source: "explicit".into(),
            source_ref: None,
            context: None,
            admission_score: 0.5,
            as_of: "t".into(),
            valid_until: None,
        };
        let err = PersonaStore::put_stable_entry(&store, &entry, "agent").unwrap_err();
        assert_eq!(err.code, ErrorCode::Internal, "{err}");
    }
}

#[path = "sqlite_persona_tests_context_tests.rs"]
mod context_tests;
