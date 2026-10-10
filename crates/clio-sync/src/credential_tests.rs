// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the sync server's credential edge: the fail-closed store
//! load, digest resolution, and the `401` matrix.
//!
//! ## Owns
//! - Behavior coverage for `credential`
//!
//! ## Does not own
//! - Where a resolved scope is applied to a bank (`server_scope_tests`)
//! - The record shape and its validators (`clio-config` tests)
//!
//! ## Boundary
//!
//! Tests here break when credential resolution changes; they never change it.
//!
//! Keep this module focused on behavior coverage for `credential`.

use super::*;
use crate::credential_fixtures::{Scratch, record, secret};
use clio_types::ErrorCode;

#[test]
fn a_loaded_record_resolves_to_its_own_peer() {
    let scratch = Scratch::new("resolve");
    let secret = secret("a");
    scratch.write(&[record("ci", &secret, &["demo"])]);
    let store = CredentialStore::load(scratch.path()).expect("load");

    let peer = store.resolve(&secret).expect("resolves");
    assert_eq!(peer.label, "ci");
    assert_eq!(peer.banks, vec!["demo".to_owned()]);
    assert!(peer.allows("demo"));
    assert!(!peer.allows("other"));
}

#[test]
fn each_record_keeps_its_own_banks() {
    let scratch = Scratch::new("multi");
    let ci = secret("ci");
    let bot = secret("bot");
    scratch.write(&[
        record("ci", &ci, &["demo", "logs"]),
        record("bot", &bot, &["scratch"]),
    ]);
    let store = CredentialStore::load(scratch.path()).expect("load");

    let ci = store.resolve(&ci).expect("ci resolves");
    let bot = store.resolve(&bot).expect("bot resolves");
    assert!(ci.allows("logs"));
    assert!(bot.allows("scratch"));
    assert!(!bot.allows("logs"), "banks do not leak between records");
}

#[test]
fn an_unknown_prefix_or_same_length_secret_resolves_to_nothing() {
    let scratch = Scratch::new("unknown");
    let known = secret("known");
    let other = secret("other");
    assert_eq!(known.len(), other.len(), "fixture must be same-length");
    scratch.write(&[record("ci", &known, &["demo"])]);
    let store = CredentialStore::load(scratch.path()).expect("load");

    assert!(store.resolve(&other).is_none());
    assert!(store.resolve(&known[..known.len() - 1]).is_none());
    assert!(store.resolve("").is_none());
}

#[test]
fn resolution_never_returns_a_secret_or_a_digest() {
    let scratch = Scratch::new("nosecret");
    let secret = secret("s");
    let stored = record("ci", &secret, &["demo"]);
    scratch.write(std::slice::from_ref(&stored));
    let store = CredentialStore::load(scratch.path()).expect("load");

    let peer = store.resolve(&secret).expect("resolves");
    let rendered = format!("{peer:?}");
    assert!(!rendered.contains(&secret), "{rendered}");
    assert!(!rendered.contains(&stored.secret_sha256), "{rendered}");
}

#[test]
fn an_absent_store_path_is_refused_with_the_fix() {
    let scratch = Scratch::new("absent");
    let err = CredentialStore::load(scratch.path()).expect_err("refused");
    assert_eq!(err.code, ErrorCode::ConfigCorrupt);
    assert!(err.message.contains("does not exist"), "{}", err.message);
    assert!(err.message.contains("auth token add"), "{}", err.message);
}

#[test]
fn a_malformed_store_is_refused() {
    let scratch = Scratch::new("malformed");
    scratch.write_raw("not json at all");
    let err = CredentialStore::load(scratch.path()).expect_err("refused");
    assert_eq!(err.code, ErrorCode::ConfigCorrupt);
}

#[test]
fn an_empty_file_is_refused() {
    let scratch = Scratch::new("empty");
    scratch.write_raw("");
    let err = CredentialStore::load(scratch.path()).expect_err("refused");
    assert_eq!(err.code, ErrorCode::ConfigCorrupt);
}

#[test]
fn an_empty_store_is_refused_because_it_authorizes_nobody() {
    let scratch = Scratch::new("norecords");
    scratch.write(&[]);
    let err = CredentialStore::load(scratch.path()).expect_err("refused");
    assert_eq!(err.code, ErrorCode::ConfigCorrupt);
    assert!(err.message.contains("holds no records"), "{}", err.message);
}

#[test]
#[cfg(unix)]
fn a_world_readable_store_is_refused_before_it_is_parsed() {
    let scratch = Scratch::new("loose");
    scratch.write_raw("[]");
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(scratch.path(), std::fs::Permissions::from_mode(0o644))
            .expect("loosen");
    }
    let err = CredentialStore::load(scratch.path()).expect_err("refused");
    assert_eq!(err.code, ErrorCode::Forbidden);
    assert!(err.message.contains("owner-only"), "{}", err.message);
}

#[test]
fn no_credential_source_in_dev_mode_is_unbound() {
    let outcome = authorize_request(None, true, None).expect("unbound");
    assert!(matches!(outcome, AuthOutcome::Unbound));
    // Even with a header present: with nothing to compare against there is no
    // credential, so the dev-mode answer is the same.
    let with_header = authorize_request(None, true, Some("Bearer anything")).expect("unbound");
    assert!(matches!(with_header, AuthOutcome::Unbound));
}

#[test]
fn a_configured_source_requires_a_credential_even_in_dev_mode() {
    let store = CredentialStore::from_records(vec![record("ci", &secret("s"), &["demo"])]);
    for header in [None, Some("Bearer not-the-secret")] {
        assert_eq!(
            authorize_request(Some(&store), true, header).unwrap_err(),
            401,
            "header {header:?} must be 401"
        );
    }
}

#[test]
fn a_missing_or_malformed_header_is_401_with_a_store() {
    let store = CredentialStore::from_records(vec![record("ci", &secret("m"), &["demo"])]);
    for header in [
        None,
        Some(""),
        Some("not-a-scheme"),
        Some("Basic dXNlcjpwYXNz"),
        Some("Bearer "),
        Some("bearer lowercase-scheme"),
    ] {
        assert_eq!(
            authorize_request(Some(&store), false, header).unwrap_err(),
            401,
            "header {header:?} must be 401"
        );
    }
}

#[test]
fn no_credential_source_outside_dev_mode_is_401() {
    assert_eq!(authorize_request(None, false, None).unwrap_err(), 401);
    assert_eq!(
        authorize_request(None, false, Some("Bearer anything")).unwrap_err(),
        401
    );
}

#[test]
fn an_unknown_secret_is_401_and_the_answer_matches_a_missing_one() {
    let store = CredentialStore::from_records(vec![record("ci", &secret("m"), &["demo"])]);
    let unknown =
        authorize_request(Some(&store), false, Some("Bearer not-the-secret")).unwrap_err();
    let missing = authorize_request(Some(&store), false, None).unwrap_err();
    assert_eq!(unknown, missing);
    assert_eq!(unknown, 401);
}

#[test]
fn a_record_secret_resolves_to_its_record_and_no_bank_beyond_it() {
    let record_secret = secret("solo");
    let store = CredentialStore::from_records(vec![record("ci", &record_secret, &["demo"])]);
    let bound = authorize_request(
        Some(&store),
        false,
        Some(&format!("Bearer {record_secret}")),
    )
    .expect("record bound");
    match bound {
        AuthOutcome::Bound(peer) => {
            assert_eq!(peer.label, "ci");
            assert!(peer.allows("demo"));
            assert!(
                !peer.allows("outside-the-record-list"),
                "a valid record cannot widen its bank list"
            );
        }
        AuthOutcome::Unbound => panic!("a matching record is a credential source"),
    }
}
