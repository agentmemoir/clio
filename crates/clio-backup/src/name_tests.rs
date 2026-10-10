// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the member-name validation gate.
//!
//! ## Owns
//! - The table of rejected and accepted member names (T03/T03b)
//!
//! ## Does not own
//! - Container or manifest tests (see the sibling `*_tests.rs` files)
//!
//! ## Boundary
//!
//! Keep this file focused on `name::validate_member_name` behavior.

use super::validate_member_name;
use clio_types::ErrorCode;

/// T03: every rejected name gets a specific `invalid archive member name`
/// error with the reason in the message.
#[test]
fn t03_name_gate_rejects_table() {
    let cases: &[(&str, &str)] = &[
        ("../escape", "'..' path segment"),
        ("a/../../b", "'..' path segment"),
        ("/abs", "starts with '/'"),
        ("a\\b", "backslash"),
        ("a//b", "empty path segment"),
        ("./x", "'.' path segment"),
        ("a/", "empty path segment"),
        ("", "the name is empty"),
        ("a\nb", "outside printable ASCII"),
        ("a\tb", "outside printable ASCII"),
        ("caf\u{e9}", "outside printable ASCII"),
        ("a\u{1f600}b", "outside printable ASCII"),
    ];

    for (name, needle) in cases {
        let err = validate_member_name(name).expect_err(name);
        assert_eq!(err.code, ErrorCode::InvalidArgument, "case {name:?}");
        assert!(
            err.message.starts_with("invalid archive member name"),
            "case {name:?}: {}",
            err.message
        );
        assert!(
            err.message.contains(needle),
            "case {name:?}: {} lacks {needle:?}",
            err.message
        );
    }
}

/// T03b: legal names pass, including exactly 100 bytes and nested paths;
/// 101 bytes is refused with the length named.
#[test]
fn t03b_name_gate_accepts_table() {
    for name in [
        "clio-backup-manifest.json",
        "keys.kms.db",
        "dump/tables/000_items.txt",
        "a",
        "0",
        "a b/c d",
        &"a".repeat(super::MAX_NAME_BYTES),
    ] {
        validate_member_name(name)
            .unwrap_or_else(|e| panic!("expected {name:?} to be accepted: {e}"));
    }

    let err = validate_member_name(&format!("{}b", "a".repeat(super::MAX_NAME_BYTES)))
        .expect_err("101 bytes must be rejected");
    assert!(
        err.message
            .contains("is 101 bytes, over the 100-byte limit"),
        "{}",
        err.message
    );
}
