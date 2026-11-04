// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the startup warning about an
//! encrypted but unverified Postgres connection: that
//! `sslmode=encrypt-no-verify` produces it, in either letter case, that every
//! mode which checks the certificate produces none, that a string naming no
//! mode produces none whether its host is local or remote, and that the
//! emission runs its one warning path and is silent thereafter.
//!
//! ## Owns
//! - The warning raised for `encrypt-no-verify`
//! - The silence for `disable`, `prefer`, and `verify-full`
//! - The silence for a string naming no mode, and for one that does not parse
//! - The emission, both its first call and its already-taken path
//!
//! ## Does not own
//! - How `sslmode` is spelled or defaulted (`pg_url` owns that)
//! - The connector each mode selects (`pg_tls` owns that)
//!
//! ## Boundary
//!
//! Changes to the warning text or to which mode raises it should normally
//! require changes here. Changes to the connection itself should not.
//!
//! Keep this module focused on the warning decision and nothing else.

use super::*;

#[test]
fn lowercase_encrypt_no_verify_names_the_mode_and_the_safe_alternative() {
    let url = "postgres://db.example.com/memories?sslmode=encrypt-no-verify";
    let message = encrypt_no_verify_warning(url).expect("encrypt-no-verify must warn");
    assert!(message.contains("encrypt-no-verify"));
    assert!(message.contains("not validated"));
    assert!(message.contains("verify-full"));
}

#[test]
fn mixed_case_encrypt_no_verify_warns_with_the_same_text() {
    let url = "postgres://db.example.com/memories?sslmode=Encrypt-No-Verify";
    let message = encrypt_no_verify_warning(url).expect("Encrypt-No-Verify must warn");
    assert!(message.contains("encrypt-no-verify"));
    assert!(message.contains("not validated"));
    assert!(message.contains("verify-full"));
}

#[test]
fn modes_that_check_or_skip_encryption_do_not_warn() {
    for mode in ["disable", "prefer", "verify-full"] {
        let url = format!("postgres://db.example.com/memories?sslmode={mode}");
        assert!(
            encrypt_no_verify_warning(&url).is_none(),
            "{mode} must not warn"
        );
    }
}

#[test]
fn a_string_naming_no_mode_does_not_warn_for_a_local_or_remote_host() {
    assert!(encrypt_no_verify_warning("postgres://localhost/memories").is_none());
    assert!(encrypt_no_verify_warning("postgres://db.example.com/memories").is_none());
}

#[test]
fn a_string_that_does_not_parse_does_not_warn() {
    assert!(encrypt_no_verify_warning("this is not a connection string").is_none());
}

#[test]
fn the_emission_warns_and_then_takes_the_already_warned_path() {
    let url = "postgres://db.example.com/memories?sslmode=encrypt-no-verify";
    warn_encrypt_no_verify(url);
    warn_encrypt_no_verify(url);
}

#[test]
fn the_emission_is_silent_for_a_checked_mode_and_for_a_string_naming_none() {
    warn_encrypt_no_verify("postgres://db.example.com/memories?sslmode=verify-full");
    warn_encrypt_no_verify("postgres://db.example.com/memories");
}
