// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the subject-shred reactivation guard tests:
//! a non-fail-closed DEK provider must not be able to reactivate a
//! crypto-shredded subject through any item write path.
//!
//! ## Owns
//! - `FailOpenKms` test double and shred-guard scenarios (SQLite + Postgres)
//!
//! ## Does not own
//! - The guard SQL and named error (`item_persist`)
//! - The erasure transaction under test (`sqlite_erase` / `postgres_erase`)
//!
//! ## Boundary
//!
//! Changes to the shred guard should normally update tests here.
//! Changes to erasure scenarios should not require changes here.
//!
//! Test-only. Never compiled into production builds.

use crate::compliance_store::ComplianceStore;
use crate::dek::{CiphertextEnvelope, DekProvider};
use crate::erase_test_support::{memory_item, spec, unique};
use crate::item_persist::shredded_subject_err;
use crate::postgres::PostgresStore;
use crate::sqlite::SqliteStore;
use crate::store::Store;
use base64::Engine as _;
use clio_types::{AmError, ErrorCode};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// DEK provider that behaves like a KMS whose "destroyed" keys resurrect from
/// a stale backup: `destroy_dek` is observable (so the erase probe passes) but
/// `ensure_dek`/`encrypt` later recreate a fresh active key for the same
/// subject. Exactly the fail-open contract the storage-layer guard backstops.
#[derive(Default)]
struct FailOpenKms {
    keys: Mutex<HashMap<String, KeyState>>,
}

#[derive(Clone)]
enum KeyState {
    Active([u8; 32]),
    /// Destroyed; `ensure_dek` fails once (erase-time probe), then the stale
    /// backup restores an active key (the fail-open behavior under test).
    Destroyed,
}

impl FailOpenKms {
    /// Active key for sealing; a destroyed key silently resurrects (fail open).
    fn key(&self, subject_id: &str) -> (u32, [u8; 32]) {
        let mut keys = self.keys.lock().expect("fail-open kms keys");
        match keys.get(subject_id) {
            Some(KeyState::Active(k)) => (1, *k),
            Some(KeyState::Destroyed) | None => {
                keys.insert(subject_id.to_owned(), KeyState::Active([7u8; 32]));
                (1, [7u8; 32])
            }
        }
    }

    /// Erase-time probe: fail closed exactly once after `destroy_dek`.
    fn probe(&self, subject_id: &str) -> Result<String, AmError> {
        let mut keys = self.keys.lock().expect("fail-open kms keys");
        if matches!(keys.get(subject_id), Some(KeyState::Destroyed)) {
            keys.insert(subject_id.to_owned(), KeyState::Active([9u8; 32]));
            return Err(AmError::new(
                ErrorCode::ErasedSubject,
                format!("DEK for subject `{subject_id}` has been destroyed"),
            ));
        }
        Ok(format!("local:{subject_id}:v1"))
    }
}

impl DekProvider for FailOpenKms {
    fn ensure_dek(&self, subject_id: &str) -> Result<String, AmError> {
        if subject_id.is_empty() {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "subject_id must be non-empty",
            ));
        }
        {
            let keys = self.keys.lock().expect("fail-open kms keys");
            if matches!(keys.get(subject_id), Some(KeyState::Destroyed)) {
                drop(keys);
                return self.probe(subject_id);
            }
        }
        let (version, _) = self.key(subject_id);
        Ok(format!("local:{subject_id}:v{version}"))
    }

    fn encrypt(&self, subject_id: &str, plaintext: &[u8]) -> Result<CiphertextEnvelope, AmError> {
        let (version, key) = self.key(subject_id);
        let cipher: Vec<u8> = plaintext
            .iter()
            .enumerate()
            .map(|(i, b)| b ^ key[i % 32])
            .collect();
        Ok(CiphertextEnvelope {
            subject_id: subject_id.to_owned(),
            key_version: version,
            nonce_b64: String::new(),
            ciphertext_b64: base64::engine::general_purpose::STANDARD.encode(&cipher),
        })
    }

    fn decrypt(&self, envelope: &CiphertextEnvelope) -> Result<Vec<u8>, AmError> {
        {
            let keys = self.keys.lock().expect("fail-open kms keys");
            if matches!(keys.get(&envelope.subject_id), Some(KeyState::Destroyed)) {
                return Err(AmError::new(
                    ErrorCode::ErasedSubject,
                    format!(
                        "DEK for subject `{}` has been destroyed",
                        envelope.subject_id
                    ),
                ));
            }
        }
        let (_, key) = self.key(&envelope.subject_id);
        let raw = base64::engine::general_purpose::STANDARD
            .decode(&envelope.ciphertext_b64)
            .map_err(|e| AmError::new(ErrorCode::Internal, e.to_string()))?;
        Ok(raw
            .iter()
            .enumerate()
            .map(|(i, b)| b ^ key[i % 32])
            .collect())
    }

    fn destroy_dek(&self, subject_id: &str) -> Result<(), AmError> {
        self.keys
            .lock()
            .expect("fail-open kms keys")
            .insert(subject_id.to_owned(), KeyState::Destroyed);
        Ok(())
    }

    fn diagnose_masked(&self, subject_id: &str) -> String {
        format!("fail-open kms for `{subject_id}`")
    }

    fn blind_index_key(&self) -> Result<[u8; 32], AmError> {
        // Deterministic per test process: this double exists to probe failure
        // classification, not to model key storage.
        Ok([0x7b; 32])
    }

    fn ensure_blind_index_key(&self) -> Result<[u8; 32], AmError> {
        Ok([0x7b; 32])
    }
}

fn open_sqlite() -> crate::sqlite::SqliteStore {
    let kms = Arc::new(FailOpenKms::default()) as Arc<dyn DekProvider>;
    SqliteStore::open_in_memory(kms).expect("sqlite")
}

#[test]
fn sqlite_fail_open_kms_cannot_reactivate_shredded_subject() {
    let store = open_sqlite();
    let bank = unique("shred-b");
    let subject = unique("shred-s");
    store
        .create_memory_item(&memory_item(&bank, &unique("i-a"), &subject), "tester")
        .unwrap();
    store
        .erase_subject(&spec(&bank, &subject, &unique("req")))
        .unwrap();
    let rejected_item_id = unique("i-b");
    let err = store
        .create_memory_item(&memory_item(&bank, &rejected_item_id, &subject), "tester")
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::ErasedSubject);
    assert_eq!(err, shredded_subject_err(&subject));
    assert!(
        store
            .get_memory_item(&bank, &rejected_item_id)
            .unwrap()
            .is_none()
    );
    // The shred marker survives the refused write.
    let still_shredded: i64 = store
        .with_conn(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM subject_keys WHERE subject_id = ?1 \
                 AND status = 'shredded' AND shredded_at IS NOT NULL",
                [&subject],
                |r| r.get(0),
            )
            .map_err(|e| AmError::new(ErrorCode::Internal, e.to_string()))
        })
        .unwrap();
    assert_eq!(still_shredded, 1);
    // Compliant behavior is unchanged: a fresh subject still writes.
    let fresh = unique("fresh-s");
    store
        .create_memory_item(&memory_item(&bank, &unique("i-c"), &fresh), "tester")
        .unwrap();
}

#[test]
fn pg_fail_open_kms_cannot_reactivate_shredded_subject() {
    let _guard = crate::pg_lock::lock_pg_test();
    let url = crate::db_url::postgres_url_from_env();
    let kms = Arc::new(FailOpenKms::default()) as Arc<dyn DekProvider>;
    let store = match PostgresStore::open(url.as_str(), Arc::clone(&kms)) {
        Ok(store) => store,
        Err(err) => {
            crate::pg_skip::unreachable_pg_or_skip("pg shred guard test", &err);
            return;
        }
    };
    let bank = unique("pg-shred-b");
    let subject = unique("pg-shred-s");
    store
        .create_memory_item(&memory_item(&bank, &unique("i-a"), &subject), "tester")
        .unwrap();
    store
        .erase_subject(&spec(&bank, &subject, &unique("req")))
        .unwrap();
    let rejected_item_id = unique("i-b");
    let err = store
        .create_memory_item(&memory_item(&bank, &rejected_item_id, &subject), "tester")
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::ErasedSubject);
    assert_eq!(err, shredded_subject_err(&subject));
    assert!(
        store
            .get_memory_item(&bank, &rejected_item_id)
            .unwrap()
            .is_none()
    );
    // Cleanup test rows (queue rows first: raw item deletes leave them behind).
    store
        .test_execute(&format!(
            "DELETE FROM index_pending WHERE bank_id = '{bank}'; \
             DELETE FROM items WHERE bank_id = '{bank}'; \
             DELETE FROM subject_keys WHERE subject_id = '{subject}';"
        ))
        .unwrap();
}
