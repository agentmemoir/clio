// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the token store: record round-trips, digest and secret
//! generation, label/actor/bank validation, and the fail-closed load rules.
//!
//! ## Owns
//! - Behavior tests for `auth_tokens`
//!
//! ## Does not own
//! - The store implementation under test
//! - CLI-level tests (they live in the CLI crate)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

/// Unique per-call scratch path; the nanos + distinct names keep parallel
/// tests off each other's files (the repo's own fixture pattern).
fn tmp(name: &str) -> PathBuf {
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    std::env::temp_dir().join(format!("am-auth-tokens-{name}-{n}.json"))
}

/// The env lookup fixture, shared with the integrity tests module.
pub(super) fn lookup<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
    move |key: &str| {
        pairs
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| (*v).to_owned())
    }
}

pub(super) fn sample_record(label: &str) -> AuthTokenRecord {
    AuthTokenRecord {
        label: label.to_owned(),
        secret_sha256: digest_hex("irrelevant-digest-input"),
        actor: Actor::Agent,
        banks: vec!["demo".to_owned()],
        created_at: utc_stamp(1_766_000_000),
    }
}

#[test]
fn record_round_trips_through_json() {
    let record = sample_record("ci-runner");
    let text = serde_json::to_string(&record).expect("serialize");
    let back: AuthTokenRecord = serde_json::from_str(&text).expect("deserialize");
    assert_eq!(back, record);
    // The serialization carries no secret field: the digest is the only
    // secret-derived value the record can hold.
    assert!(!text.contains("secret\""));
    assert!(text.contains("secret_sha256"));
}

#[test]
fn unknown_fields_are_refused() {
    let text = r#"{"label":"a","secret_sha256":"0","actor":"agent","banks":["b"],
        "created_at":"2026-01-01T00:00:00Z","extra":1}"#;
    let err = serde_json::from_str::<AuthTokenRecord>(text).expect_err("unknown field");
    assert!(err.to_string().contains("unknown field"), "{err}");
}

#[test]
fn digest_is_the_sha256_hex_of_the_secret_bytes() {
    // Fixed expected value: SHA-256("abc"), independent of the host.
    assert_eq!(
        digest_hex("abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(digest_hex("").len(), 64);
}

#[test]
fn generated_secret_has_the_recorded_shape_and_entropy() {
    let secret = generate_secret();
    assert_eq!(secret.len(), 43);
    assert!(
        secret
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    );
    // Two generations differ, so the CSPRNG is really in the path.
    assert_ne!(generate_secret(), secret);
    // And a secret is not its own digest.
    assert_ne!(digest_hex(&secret), secret);
}

#[test]
fn ct_eq_matches_only_equal_length_equal_bytes() {
    assert!(ct_eq(b"Bearer token", b"Bearer token"));
    assert!(!ct_eq(b"Bearer token", b"Bearer tokeN"));
    assert!(!ct_eq(b"Bearer token", b"Bearer tokenx"));
    assert!(!ct_eq(b"", b"x"));
    assert!(ct_eq(b"", b""));
}

#[test]
fn label_rules_are_bounded() {
    assert!(validate_label("ci-runner").is_ok());
    assert!(validate_label("a").is_ok());
    assert!(validate_label("A.b_2-").is_ok());
    assert!(validate_label("").is_err());
    assert!(validate_label(&"x".repeat(65)).is_err());
    assert!(validate_label(&"x".repeat(64)).is_ok());
    assert!(validate_label("has space").is_err());
    assert!(validate_label("slash/ed").is_err());
    assert!(validate_label("uni\u{00e9}code").is_err());
}

#[test]
fn banks_need_one_entry_and_collapse_duplicates() {
    assert!(validate_banks(&[]).is_err());
    assert!(validate_banks(&[String::from("  ")]).is_err());
    let banks = validate_banks(&[
        "demo".into(),
        " demo ".into(),
        String::new(),
        "other".into(),
    ])
    .expect("ok");
    assert_eq!(banks, vec!["demo".to_owned(), "other".to_owned()]);
}

#[test]
fn actor_errors_use_the_shared_parser_message() {
    let err = Actor::parse("robot").expect_err("invalid actor");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert!(err.message.contains("agent|harness|user|system"), "{err}");
}

#[test]
fn new_auth_token_stores_only_the_digest() {
    let (record, secret) = new_auth_token("lab", "harness", &["demo".to_owned()]).expect("valid");
    assert_eq!(record.label, "lab");
    assert_eq!(record.actor, Actor::Harness);
    assert_eq!(record.banks, vec!["demo".to_owned()]);
    assert_eq!(record.secret_sha256, digest_hex(&secret));
    // created_at is host-clocked, so assert the shape, never the value.
    assert_eq!(record.created_at.len(), 20);
    assert!(record.created_at.ends_with('Z'));
}

#[test]
fn utc_stamp_matches_a_known_second() {
    assert_eq!(utc_stamp(0), "1970-01-01T00:00:00Z");
    assert_eq!(utc_stamp(1_766_000_000), "2025-12-17T19:33:20Z");
}

#[test]
fn add_refuses_a_taken_label_and_keeps_the_record() {
    let records = vec![sample_record("dup"), sample_record("keep")];
    let err = add_record(records.clone(), sample_record("dup")).expect_err("taken");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert!(err.message.contains("already exists"), "{err}");
    let grown = add_record(records.clone(), sample_record("new")).expect("free");
    assert_eq!(grown.len(), 3);
    assert_eq!(records.len(), 2);
}

#[test]
fn remove_refuses_an_unknown_label_and_removes_the_known_one() {
    let records = vec![sample_record("a"), sample_record("b")];
    let err = remove_record(records.clone(), "nope").expect_err("unknown");
    assert_eq!(err.code, ErrorCode::NotFound);
    let kept = remove_record(records, "a").expect("known");
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].label, "b");
}

#[test]
fn load_of_a_missing_file_is_an_empty_store() {
    assert_eq!(load_auth_tokens(&tmp("absent")).expect("empty"), Vec::new());
}

#[test]
fn save_then_load_round_trips_and_stays_owner_only() {
    let path = tmp("round-trip");
    let (record, secret) =
        new_auth_token("one", "user", &["demo".to_owned(), "other".to_owned()]).expect("valid");
    let records = add_record(Vec::new(), record).expect("add");
    save_auth_tokens(&path, &records).expect("save");
    // The persisted bytes carry no plaintext secret, only its digest.
    let bytes = fs::read_to_string(&path).expect("read");
    assert!(!bytes.contains(&secret), "plaintext secret on disk");
    assert!(bytes.contains(&digest_hex(&secret)), "digest missing");
    assert_eq!(load_auth_tokens(&path).expect("load"), records);
    // Shape assertion: the file grants nothing to group or other, without
    // pinning the host's exact mode value.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&path).expect("meta").permissions().mode();
        assert_eq!(mode & 0o77, 0, "not owner-only: {mode:o}");
    }
    let _ = fs::remove_file(&path);
}

#[test]
fn save_preserves_the_records_it_was_not_given_a_chance_to_drop() {
    // A two-record store rewritten after one removal keeps the other record.
    let path = tmp("preserve");
    let records = vec![sample_record("a"), sample_record("b")];
    save_auth_tokens(&path, &records).expect("save");
    let loaded = load_auth_tokens(&path).expect("load");
    let kept = remove_record(loaded, "a").expect("remove");
    save_auth_tokens(&path, &kept).expect("save again");
    assert_eq!(load_auth_tokens(&path).expect("reload"), kept);
    let _ = fs::remove_file(&path);
}

#[test]
fn empty_and_malformed_files_are_refused() {
    // The permission check runs first, so the scratch files are made
    // owner-only here to reach the parse checks.
    #[cfg(unix)]
    let tighten = |path: &Path| {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).expect("tighten");
    };
    #[cfg(not(unix))]
    let tighten = |path: &Path| {
        let _ = path;
    };

    let empty = tmp("empty");
    fs::write(&empty, "").expect("write empty");
    tighten(&empty);
    let err = load_auth_tokens(&empty).expect_err("empty");
    assert_eq!(err.code, ErrorCode::ConfigCorrupt);
    assert!(err.message.contains("malformed or empty"), "{err}");

    let truncated = tmp("truncated");
    fs::write(&truncated, r#"[{"label":"a""#).expect("write truncated");
    tighten(&truncated);
    assert_eq!(
        load_auth_tokens(&truncated).expect_err("truncated").code,
        ErrorCode::ConfigCorrupt
    );

    let wrong_shape = tmp("shape");
    fs::write(&wrong_shape, r#"{"not":"an array"}"#).expect("write object");
    tighten(&wrong_shape);
    assert_eq!(
        load_auth_tokens(&wrong_shape)
            .expect_err("object root")
            .code,
        ErrorCode::ConfigCorrupt
    );
    let _ = fs::remove_file(&empty);
    let _ = fs::remove_file(&truncated);
    let _ = fs::remove_file(&wrong_shape);
}

#[test]
fn invalid_loaded_records_are_refused() {
    let mut record = sample_record("bad label!");
    let path = tmp("invalid-label");
    save_auth_tokens(&path, &[record.clone()]).expect("save ignores CLI-level labels");
    let err = load_auth_tokens(&path).expect_err("bad label");
    assert_eq!(err.code, ErrorCode::ConfigCorrupt);
    assert!(err.message.contains("bad label!"), "{err}");

    record = sample_record("ok");
    record.banks = Vec::new();
    let path = tmp("no-banks");
    save_auth_tokens(&path, &[record]).expect("save");
    assert_eq!(
        load_auth_tokens(&path).expect_err("no banks").code,
        ErrorCode::ConfigCorrupt
    );

    let record = sample_record("ok");
    let path = tmp("bad-digest");
    save_auth_tokens(&path, &[record]).expect("save");
    // Overwrite the digest with a non-hex value to reach the digest check.
    let poisoned = fs::read_to_string(&path)
        .expect("read")
        .replace("secret_sha256\": \"", "secret_sha256\": \"xyz");
    fs::write(&path, &poisoned).expect("poison");
    assert_eq!(
        load_auth_tokens(&path).expect_err("bad digest").code,
        ErrorCode::ConfigCorrupt
    );
    let _ = fs::remove_file(&path);
}

#[test]
fn a_repeated_label_in_the_file_is_refused() {
    let path = tmp("repeat-label");
    save_auth_tokens(&path, &[sample_record("same"), sample_record("same")])
        .expect("save writes what it is told");
    let err = load_auth_tokens(&path).expect_err("repeated label");
    assert!(err.message.contains("repeated label"), "{err}");
    let _ = fs::remove_file(&path);
}

#[test]
fn a_file_group_or_other_can_read_is_refused() {
    let path = tmp("perms");
    save_auth_tokens(&path, &[sample_record("a")]).expect("save");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).expect("loosen");
        let err = load_auth_tokens(&path).expect_err("world-readable");
        assert_eq!(err.code, ErrorCode::Forbidden);
        assert!(err.message.contains("owner-only"), "{err}");
    }
    let _ = fs::remove_file(&path);
}
