// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the gist decoding of the store-path write: an absent gist
//! stays absent, and a blank gist is refused before anything is stored.
//!
//! ## Owns
//! - Assertions over `gist_of`
//!
//! ## Does not own
//! - The store path itself (see `store_write`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;
use serde_json::json;

#[test]
fn an_absent_gist_stays_absent() {
    assert_eq!(gist_of(&json!({})).expect("absent gist"), None);
}

#[test]
fn a_non_blank_gist_is_kept_as_given() {
    assert_eq!(
        gist_of(&json!({"gist": "Tom walks on Fridays"})).expect("gist"),
        Some("Tom walks on Fridays".to_owned())
    );
}

#[test]
fn an_empty_or_whitespace_only_gist_is_refused() {
    for blank in ["", "   ", "\t\n"] {
        let error = gist_of(&json!({ "gist": blank })).expect_err("blank gist");
        assert_eq!(error.code, ErrorCode::InvalidArgument, "{blank:?}");
        assert!(
            error
                .message
                .contains("must not be empty or whitespace-only"),
            "{blank:?}: {}",
            error.message
        );
    }
}
