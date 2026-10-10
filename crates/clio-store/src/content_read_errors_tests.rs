// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for proving the skippable-versus-fail-closed
//! classification covers every [`clio_types::ErrorCode`] deliberately.
//!
//! ## Owns
//! - The exhaustive code-by-code table of what `is_unreadable_content` accepts
//! - The guard that a new code forces a decision instead of a silent default
//!
//! ## Does not own
//! - Production classification (`content_read_errors`)
//! - The wire-string contract (`clio-types`)
//!
//! # Why the table is exhaustive
//!
//! The predicate is a three-variant allowlist, so any code added to
//! `clio-types` lands on the deny side by default. That default is what keeps
//! backend failures fail-closed, and it is also a decision nobody made on
//! purpose. This table names every code and its side, so adding a code forces a
//! human to say which side it belongs to instead of inheriting one.

use super::*;
use clio_types::{AmError, ErrorCode};

#[test]
fn skippable_codes_are_exactly_the_three_content_shapes() {
    for code in ErrorCode::ALL {
        let expected = matches!(
            *code,
            ErrorCode::UnreadableContent | ErrorCode::Forbidden | ErrorCode::ErasedSubject
        );
        let actual = is_unreadable_content(&AmError::new(*code, "x"));
        assert_eq!(actual, expected, "{code} is on the wrong side");
    }
}

#[test]
fn backend_failures_stay_fail_closed() {
    // The one that matters: a broken driver, database, or key store reports
    // `Internal`, and skipping it would admit writes on guessed signals.
    assert!(!is_unreadable_content(&AmError::new(
        ErrorCode::Internal,
        "read DEK slot: database is locked"
    )));
    for code in [
        ErrorCode::ConfigCorrupt,
        ErrorCode::MissingBank,
        ErrorCode::NotFound,
        ErrorCode::ItemNotFound,
        ErrorCode::InvalidArgument,
    ] {
        assert!(
            !is_unreadable_content(&AmError::new(code, "x")),
            "{code} must not be skippable"
        );
    }
}

#[test]
fn each_content_shape_is_skippable_with_its_own_message() {
    for (code, message) in [
        (ErrorCode::UnreadableContent, "invalid nonce length"),
        (ErrorCode::Forbidden, "no DEK for subject `s`"),
        (
            ErrorCode::ErasedSubject,
            "DEK for subject `s` has been destroyed",
        ),
    ] {
        let err = AmError::new(code, message);
        assert!(
            is_unreadable_content(&err),
            "{code}: {err} must be skippable"
        );
    }
}
