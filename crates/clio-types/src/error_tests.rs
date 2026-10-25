// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests error codes and envelopes.
//!
//! ## Owns
//! - Error code and envelope assertions
//!
//! ## Does not own
//! - Production behavior
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;

#[test]
fn error_display_includes_code() {
    let err = AmError::new(ErrorCode::InvalidType, "expected number");
    assert!(err.to_string().contains("invalid_type"));
}

#[test]
fn tool_result_failure_serializes_ok_false() {
    let result = ToolResult::<()>::failure(AmError::not_implemented("store"));
    let json = serde_json::to_string(&result).expect("serialize");
    assert!(json.contains("\"ok\":false"));
    assert!(json.contains("not_implemented"));
}

/// The wire-string contract, frozen.
///
/// This table is the compatibility promise. Adding a row is the only
/// supported way to add a code; changing a string is a breaking change to
/// every consumer that matched it. Because the table and
/// [`ErrorCode::ALL`] must agree on length, a new variant cannot be added to
/// one without the other — which is what stops the list going stale.
const CODE_STRINGS: &[(ErrorCode, &str)] = &[
    (ErrorCode::NotImplemented, "not_implemented"),
    (ErrorCode::InvalidArgument, "invalid_argument"),
    (ErrorCode::InvalidType, "invalid_type"),
    (ErrorCode::OutOfRange, "out_of_range"),
    (ErrorCode::UnknownProfile, "unknown_profile"),
    (ErrorCode::UnknownPath, "unknown_path"),
    (ErrorCode::MissingBank, "missing_bank"),
    (ErrorCode::NotFound, "not_found"),
    (ErrorCode::ItemNotFound, "item_not_found"),
    (
        ErrorCode::CoactivationHandoffFailed,
        "coactivation_handoff_failed",
    ),
    (ErrorCode::Forbidden, "forbidden"),
    (ErrorCode::ConfigCorrupt, "config_corrupt"),
    (ErrorCode::FactorUnavailable, "factor_unavailable"),
    (ErrorCode::WrongUpdateRule, "wrong_update_rule"),
    (ErrorCode::ErasedSubject, "erased_subject"),
    (ErrorCode::UnreadableContent, "unreadable_content"),
    (ErrorCode::QuotaExceeded, "quota_exceeded"),
    (ErrorCode::ArchiveCorrupt, "archive_corrupt"),
    (ErrorCode::StorageBusy, "storage_busy"),
    (ErrorCode::Internal, "internal"),
];

#[test]
fn all_error_codes_have_stable_strings() {
    for (code, wire) in CODE_STRINGS {
        assert_eq!(code.as_str(), *wire, "{code:?} wire string changed");
        assert_eq!(code.to_string(), *wire, "{code:?} Display drifted");
    }
}

/// The frozen table and [`ErrorCode::ALL`] describe the same set, with no
/// duplicates on either side.
#[test]
fn all_error_codes_covers_exactly_the_frozen_table() {
    assert_eq!(
        ErrorCode::ALL.len(),
        CODE_STRINGS.len(),
        "a code was added without a frozen wire string"
    );
    for code in ErrorCode::ALL {
        let occurrences = CODE_STRINGS
            .iter()
            .filter(|(listed, _)| listed == code)
            .count();
        assert_eq!(occurrences, 1, "{code:?} is not listed exactly once");
    }
}

/// `AmError` is a `std::error::Error`, so callers can use `?` into any
/// boxed-error boundary.
#[test]
fn am_error_is_a_std_error() {
    let err = AmError::new(ErrorCode::Internal, "x");
    let as_std: &dyn std::error::Error = &err;
    assert_eq!(as_std.to_string(), "internal: x");
}

#[test]
fn tool_result_from_result() {
    let ok = serde_json::to_value(ToolResult::from_result(Ok(()))).expect("ser");
    assert_eq!(ok["ok"], true);
    let err = serde_json::to_value(ToolResult::<()>::from_result(Err(AmError::new(
        ErrorCode::Forbidden,
        "no",
    ))))
    .expect("ser");
    assert_eq!(err["ok"], false);
    assert_eq!(err["code"], "forbidden");
}
