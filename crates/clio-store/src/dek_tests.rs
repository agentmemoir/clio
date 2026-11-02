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
    let env = kms.encrypt("test.field", "row", "sub-1", b"hello").unwrap();
    assert_eq!(kms.decrypt("test.field", "row", &env).unwrap(), b"hello");
    let stored = env.to_storage_string();
    let parsed = CiphertextEnvelope::from_storage_string(&stored).unwrap();
    assert_eq!(kms.decrypt("test.field", "row", &parsed).unwrap(), b"hello");
}

/// The reported defect: a ciphertext moved from one row of a subject to another
/// row of the same subject must stop authenticating.
#[test]
fn ciphertext_swapped_between_rows_of_one_subject_fails() {
    let kms = LocalDevKms::new();
    let first = kms
        .encrypt("test.field", "row-a", "sub-1", b"alpha")
        .unwrap();
    let second = kms
        .encrypt("test.field", "row-b", "sub-1", b"beta")
        .unwrap();

    let err = kms.decrypt("test.field", "row-b", &first).unwrap_err();
    assert_eq!(err.code, ErrorCode::Forbidden);
    assert!(crate::is_unreadable_content(&err), "{err:?}");

    let err = kms.decrypt("test.field", "row-a", &second).unwrap_err();
    assert_eq!(err.code, ErrorCode::Forbidden);
}

#[test]
fn ciphertext_swapped_between_fields_of_one_row_fails() {
    let kms = LocalDevKms::new();
    let proposition = kms
        .encrypt("belief.proposition", "belief-1", "sub-1", b"alpha")
        .unwrap();
    assert_eq!(
        kms.decrypt("belief.context", "belief-1", &proposition)
            .unwrap_err()
            .code,
        ErrorCode::Forbidden
    );
}

#[test]
fn a_changed_row_id_fails_to_open() {
    let kms = LocalDevKms::new();
    let env = kms
        .encrypt("test.field", "row-a", "sub-1", b"alpha")
        .unwrap();
    assert_eq!(kms.decrypt("test.field", "row-a", &env).unwrap(), b"alpha");
    assert_eq!(
        kms.decrypt("test.field", "row-b", &env).unwrap_err().code,
        ErrorCode::Forbidden
    );
}

#[test]
fn the_aad_encoding_is_unambiguous_across_field_boundaries() {
    assert_eq!(AAD_LABEL, b"clio-dek-envelope-v1");
    assert_ne!(
        envelope_aad("f", "ab", "c", 1),
        envelope_aad("f", "a", "bc", 1),
        "length prefixes must stop one field bleeding into the next"
    );
    assert_ne!(
        envelope_aad("field.a", "row", "subject", 1),
        envelope_aad("field.b", "row", "subject", 1),
        "the field role must be covered by the tag"
    );
    assert_ne!(
        envelope_aad("f", "row", "subject", 1),
        envelope_aad("f", "row", "subject", 2),
        "the key version must be covered by the tag"
    );
    let aad = envelope_aad("f", "r", "s", 7);
    assert!(aad.starts_with(AAD_LABEL));
    assert_eq!(aad[AAD_LABEL.len()], AAD_VERSION);
    assert_eq!(&aad[aad.len() - 4..], &7u32.to_be_bytes());
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
    let env = kms
        .encrypt("test.field", "row", "sub-2", b"secret")
        .unwrap();
    kms.destroy_dek("sub-2").unwrap();
    assert_eq!(
        kms.decrypt("test.field", "row", &env).unwrap_err().code,
        ErrorCode::ErasedSubject
    );
    assert_eq!(
        kms.encrypt("test.field", "row", "sub-2", b"x")
            .unwrap_err()
            .code,
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
    let mut env = kms.encrypt("test.field", "row", "sub-3", b"x").unwrap();
    env.key_version = 99;
    assert_eq!(
        kms.decrypt("test.field", "row", &env).unwrap_err().code,
        ErrorCode::Forbidden,
        "a DEK version mismatch is content, not backend"
    );
    let mut env = kms.encrypt("test.field", "row", "sub-3", b"x").unwrap();
    env.nonce_b64 = "****".into();
    assert_eq!(
        kms.decrypt("test.field", "row", &env).unwrap_err().code,
        ErrorCode::UnreadableContent,
        "an undecodable nonce must be skippable"
    );
    let mut env = kms.encrypt("test.field", "row", "sub-3", b"x").unwrap();
    env.nonce_b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, [0u8; 12]);
    assert_eq!(
        kms.decrypt("test.field", "row", &env).unwrap_err().code,
        ErrorCode::UnreadableContent,
        "a legacy 12-byte nonce must be skippable"
    );
    let mut env = kms.encrypt("test.field", "row", "sub-3", b"x").unwrap();
    let mut nonce =
        base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &env.nonce_b64).unwrap();
    nonce[23] ^= 0xff;
    env.nonce_b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, nonce);
    assert_eq!(
        kms.decrypt("test.field", "row", &env).unwrap_err().code,
        ErrorCode::Forbidden,
        "a tampered 24-byte nonce must fail authentication"
    );
    let mut env = kms.encrypt("test.field", "row", "sub-3", b"x").unwrap();
    env.ciphertext_b64 = "!!!".into();
    assert_eq!(
        kms.decrypt("test.field", "row", &env).unwrap_err().code,
        ErrorCode::UnreadableContent,
        "an undecodable ciphertext must be skippable"
    );
    assert_eq!(
        kms.decrypt(
            "test.field",
            "row",
            &CiphertextEnvelope {
                subject_id: "nope".into(),
                key_version: 1,
                nonce_b64: String::new(),
                ciphertext_b64: String::new(),
            }
        )
        .unwrap_err()
        .code,
        ErrorCode::Forbidden,
        "no DEK for the subject is content, not backend"
    );
    let mut env = kms.encrypt("test.field", "row", "sub-3", b"ok").unwrap();
    let mut raw = base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        &env.ciphertext_b64,
    )
    .unwrap();
    raw[0] ^= 0xff;
    env.ciphertext_b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, raw);
    assert_eq!(
        kms.decrypt("test.field", "row", &env).unwrap_err().code,
        ErrorCode::Forbidden
    );
}

/// An envelope sealed under the older 12-byte nonce layout cannot be opened by
/// the XChaCha20-Poly1305 cipher, so it must report as skippable content.
#[test]
fn legacy_twelve_byte_nonce_envelope_is_rejected() {
    let kms = LocalDevKms::new();
    let mut env = kms
        .encrypt("test.field", "row", "sub-legacy", b"old")
        .unwrap();
    env.nonce_b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, [7u8; 12]);
    assert_eq!(
        kms.decrypt("test.field", "row", &env).unwrap_err().code,
        ErrorCode::UnreadableContent,
        "a legacy nonce length must be skippable, never opened"
    );
}

#[test]
fn file_backed_kms_survives_a_new_instance() {
    let dir = temp_dir("survives");
    let path = dir.join("clio.kms.json");

    let writer = LocalDevKms::open_persistent(&path).unwrap();
    let env = writer
        .encrypt("test.field", "row", "sub-a", b"remember me")
        .unwrap();
    let dek_ref = writer.ensure_dek("sub-a").unwrap();
    drop(writer);

    let reader = LocalDevKms::open_persistent(&path).unwrap();
    assert_eq!(
        reader.decrypt("test.field", "row", &env).unwrap(),
        b"remember me"
    );
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
    let env = writer
        .encrypt("test.field", "row", "sub-b", b"gone")
        .unwrap();
    writer.destroy_dek("sub-b").unwrap();
    drop(writer);

    let reader = LocalDevKms::open_persistent(&path).unwrap();
    assert_eq!(
        reader.decrypt("test.field", "row", &env).unwrap_err().code,
        ErrorCode::ErasedSubject
    );
    cleanup(&dir);
}

#[test]
fn shred_is_visible_to_an_already_open_instance() {
    let dir = temp_dir("shred-open");
    let path = dir.join("clio.kms.json");
    let writer = LocalDevKms::open_persistent(&path).unwrap();
    let env = writer
        .encrypt("test.field", "row", "sub-c", b"gone")
        .unwrap();
    // The reader opened before the shred; refresh must still see it.
    let reader = LocalDevKms::open_persistent(&path).unwrap();
    assert_eq!(reader.decrypt("test.field", "row", &env).unwrap(), b"gone");
    writer.destroy_dek("sub-c").unwrap();
    assert_eq!(
        reader.decrypt("test.field", "row", &env).unwrap_err().code,
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
    let env_a = first.encrypt("test.field", "row", "sub-a", b"a").unwrap();
    let env_b = second.encrypt("test.field", "row", "sub-b", b"b").unwrap();

    let reader = LocalDevKms::open_persistent(&path).unwrap();
    assert_eq!(reader.decrypt("test.field", "row", &env_a).unwrap(), b"a");
    assert_eq!(reader.decrypt("test.field", "row", &env_b).unwrap(), b"b");
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
                    kms.encrypt("test.field", "row", &format!("sub-{writer}-{item}"), b"x")
                        .unwrap();
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
