// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for unit-testing the local DEK provider: crypto
//! round-trips, crypto-shred semantics, and file-backed persistence across
//! provider instances.
//!
//! ## Owns
//! - In-memory round-trip, destroy, and malformed-envelope cases
//! - Cross-instance persistence, refresh, and damaged key-store refusals
//!
//! ## Does not own
//! - The provider implementation (see `dek`)
//! - The cached slot map (see `dek_slots_tests`)
//! - The key-store file format (see `local_kms_db_tests`)
//!
//! ## Boundary
//!
//! Tests here break when DEK lifecycle or persistence changes; they never change
//! the implementation.
//!
//! Keep this module focused on provider behavior.

use super::*;
use crate::kms_migrate::install_test_key;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Barrier};

/// Fresh temp directory for one test, cleaned up by the caller.
///
/// A file-backed local KMS seals the per-subject keys it writes, so the shared
/// process key is installed here; without it a test would reach the machine's
/// real key store.
fn temp_dir(tag: &str) -> PathBuf {
    install_test_key();
    let dir = std::env::temp_dir().join(format!("clio-dek-{}-{tag}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn cleanup(dir: &Path) {
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn roundtrip_encrypt_decrypt() {
    let kms = LocalDevKms::new();
    let env = kms.encrypt("sub-1", b"hello").unwrap();
    assert_eq!(kms.decrypt(&env).unwrap(), b"hello");
    let stored = env.to_storage_string();
    let parsed = CiphertextEnvelope::from_storage_string(&stored).unwrap();
    assert_eq!(kms.decrypt(&parsed).unwrap(), b"hello");
}

#[test]
fn destroy_idempotent_and_absent_subject() {
    let kms = LocalDevKms::new();
    kms.destroy_dek("never-seen").unwrap();
    kms.destroy_dek("never-seen").unwrap();
    assert!(kms.diagnose_masked("never-seen").contains("shredded"));
    let dbg = format!("{kms:?}");
    assert!(dbg.contains("redacted"));
    assert!(!dbg.contains("never-seen"));
}

#[test]
fn destroy_renders_unreadable() {
    let kms = LocalDevKms::new();
    let env = kms.encrypt("sub-2", b"secret").unwrap();
    kms.destroy_dek("sub-2").unwrap();
    assert_eq!(
        kms.decrypt(&env).unwrap_err().code,
        ErrorCode::ErasedSubject
    );
    assert_eq!(
        kms.encrypt("sub-2", b"x").unwrap_err().code,
        ErrorCode::ErasedSubject
    );
    let diag = kms.diagnose_masked("sub-2");
    assert!(diag.contains("shredded"));
    assert!(!diag.contains("secret"));
}

#[test]
fn ensure_and_diagnose_paths() {
    let kms = LocalDevKms::new();
    assert!(kms.ensure_dek("").is_err());
    let r = kms.ensure_dek("s").unwrap();
    assert!(r.starts_with("local:s:v"));
    assert!(kms.diagnose_masked("s").contains("active"));
    assert!(kms.diagnose_masked("missing").contains("absent"));
    assert_eq!(
        CiphertextEnvelope::from_storage_string("not-json")
            .unwrap_err()
            .code,
        ErrorCode::UnreadableContent,
        "an unparseable envelope column must be skippable"
    );
}

/// Every damaged-envelope field must fail with `UnreadableContent`, not
/// `Internal`.
///
/// The code is the contract, not the message: the optional-row readers skip on
/// this code, so an assertion of `is_err()` alone would still pass if the code
/// regressed to `Internal` and one damaged row went back to failing every write
/// in the bank.
#[test]
fn bad_envelope_fields_fail_decrypt() {
    let kms = LocalDevKms::new();
    let mut env = kms.encrypt("sub-3", b"x").unwrap();
    env.key_version = 99;
    assert_eq!(
        kms.decrypt(&env).unwrap_err().code,
        ErrorCode::Forbidden,
        "a DEK version mismatch is content, not backend"
    );
    let mut env = kms.encrypt("sub-3", b"x").unwrap();
    env.nonce_b64 = "****".into();
    assert_eq!(
        kms.decrypt(&env).unwrap_err().code,
        ErrorCode::UnreadableContent,
        "an undecodable nonce must be skippable"
    );
    let mut env = kms.encrypt("sub-3", b"x").unwrap();
    env.nonce_b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, [0u8; 3]);
    assert_eq!(
        kms.decrypt(&env).unwrap_err().code,
        ErrorCode::UnreadableContent,
        "a wrong-length nonce must be skippable"
    );
    let mut env = kms.encrypt("sub-3", b"x").unwrap();
    env.ciphertext_b64 = "!!!".into();
    assert_eq!(
        kms.decrypt(&env).unwrap_err().code,
        ErrorCode::UnreadableContent,
        "an undecodable ciphertext must be skippable"
    );
    assert_eq!(
        kms.decrypt(&CiphertextEnvelope {
            subject_id: "nope".into(),
            key_version: 1,
            nonce_b64: String::new(),
            ciphertext_b64: String::new(),
        })
        .unwrap_err()
        .code,
        ErrorCode::Forbidden,
        "no DEK for the subject is content, not backend"
    );
    let mut env = kms.encrypt("sub-3", b"ok").unwrap();
    let mut raw = base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        &env.ciphertext_b64,
    )
    .unwrap();
    raw[0] ^= 0xff;
    env.ciphertext_b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, raw);
    assert_eq!(kms.decrypt(&env).unwrap_err().code, ErrorCode::Forbidden);
}

#[test]
fn file_backed_kms_survives_a_new_instance() {
    let dir = temp_dir("survives");
    let path = dir.join("clio.kms.json");

    let writer = LocalDevKms::open_persistent(&path).unwrap();
    let env = writer.encrypt("sub-a", b"remember me").unwrap();
    let dek_ref = writer.ensure_dek("sub-a").unwrap();
    drop(writer);

    let reader = LocalDevKms::open_persistent(&path).unwrap();
    assert_eq!(reader.decrypt(&env).unwrap(), b"remember me");
    assert_eq!(reader.ensure_dek("sub-a").unwrap(), dek_ref);
    cleanup(&dir);
}

#[test]
fn missing_key_store_starts_empty() {
    let dir = temp_dir("missing");
    let kms = LocalDevKms::open_persistent(dir.join("clio.kms.json")).unwrap();
    assert!(kms.diagnose_masked("nobody").contains("absent"));
    cleanup(&dir);
}

#[test]
fn shred_is_visible_to_a_new_instance() {
    let dir = temp_dir("shred-new");
    let path = dir.join("clio.kms.json");
    let writer = LocalDevKms::open_persistent(&path).unwrap();
    let env = writer.encrypt("sub-b", b"gone").unwrap();
    writer.destroy_dek("sub-b").unwrap();
    drop(writer);

    let reader = LocalDevKms::open_persistent(&path).unwrap();
    assert_eq!(
        reader.decrypt(&env).unwrap_err().code,
        ErrorCode::ErasedSubject
    );
    cleanup(&dir);
}

#[test]
fn shred_is_visible_to_an_already_open_instance() {
    let dir = temp_dir("shred-open");
    let path = dir.join("clio.kms.json");
    let writer = LocalDevKms::open_persistent(&path).unwrap();
    let env = writer.encrypt("sub-c", b"gone").unwrap();
    // The reader opened before the shred; refresh must still see it.
    let reader = LocalDevKms::open_persistent(&path).unwrap();
    assert_eq!(reader.decrypt(&env).unwrap(), b"gone");
    writer.destroy_dek("sub-c").unwrap();
    assert_eq!(
        reader.decrypt(&env).unwrap_err().code,
        ErrorCode::ErasedSubject
    );
    cleanup(&dir);
}

#[test]
fn subjects_from_two_writers_are_merged() {
    let dir = temp_dir("merge");
    let path = dir.join("clio.kms.json");
    let first = LocalDevKms::open_persistent(&path).unwrap();
    let second = LocalDevKms::open_persistent(&path).unwrap();
    let env_a = first.encrypt("sub-a", b"a").unwrap();
    let env_b = second.encrypt("sub-b", b"b").unwrap();

    let reader = LocalDevKms::open_persistent(&path).unwrap();
    assert_eq!(reader.decrypt(&env_a).unwrap(), b"a");
    assert_eq!(reader.decrypt(&env_b).unwrap(), b"b");
    cleanup(&dir);
}

#[test]
fn nested_key_store_creates_parent_directories() {
    let dir = temp_dir("nested");
    let path = dir.join("deep").join("keys").join("clio.kms.json");
    let kms = LocalDevKms::open_persistent(&path).unwrap();
    kms.ensure_dek("sub-d").unwrap();
    assert!(path.exists());
    cleanup(&dir);
}

/// A key-store file that is not a SQLite database must fail open, and the
/// failed open must not rewrite it.
///
/// The `.json` in the key-store name is historical; the store is SQLite. A
/// file holding anything else — for example a JSON document from an older
/// Clio — can never be read as key material, so opening fails closed rather
/// than silently discarding or reusing it.
#[test]
fn a_key_store_file_that_is_not_a_sqlite_database_fails_closed() {
    let dir = temp_dir("not-sqlite");
    let path = dir.join("clio.kms.json");
    let planted = br#"{"version":1,"slots":[]}"#;
    fs::write(&path, planted).unwrap();

    assert!(LocalDevKms::open_persistent(&path).is_err());

    assert_eq!(fs::read(&path).unwrap(), planted.to_vec());
    cleanup(&dir);
}

#[test]
fn concurrent_kms_instances_keep_every_key() {
    let dir = temp_dir("concurrent");
    let path = dir.join("clio.kms.json");
    let writers = 4usize;
    let per_writer = 8usize;
    let barrier = Arc::new(Barrier::new(writers));
    let handles: Vec<_> = (0..writers)
        .map(|writer| {
            let path = path.clone();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                let kms = LocalDevKms::open_persistent(&path).unwrap();
                barrier.wait();
                for item in 0..per_writer {
                    kms.encrypt(&format!("sub-{writer}-{item}"), b"x").unwrap();
                }
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }
    let reader = LocalDevKms::open_persistent(&path).unwrap();
    for writer in 0..writers {
        for item in 0..per_writer {
            assert!(
                reader.ensure_dek(&format!("sub-{writer}-{item}")).is_ok(),
                "a concurrent writer's DEK was lost"
            );
        }
    }
    cleanup(&dir);
}
