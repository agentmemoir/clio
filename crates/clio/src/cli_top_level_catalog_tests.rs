// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests that the JSON catalog is requested only by arguments
//! before the `--` delimiter.
//!
//! ## Owns
//! - Assertions over `wants_json_catalog` at and beyond the delimiter
//!
//! ## Does not own
//! - The catalog grammar under test (`cli_top_level`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::super::wants_json_catalog;

/// The catalog is requested only by arguments before the `--` delimiter.
#[test]
fn the_json_catalog_is_requested_only_before_the_delimiter() {
    let tokens = |values: &[&str]| {
        values
            .iter()
            .map(|value| (*value).to_owned())
            .collect::<Vec<_>>()
    };
    assert!(wants_json_catalog(&tokens(&["health", "--json"])));
    assert!(wants_json_catalog(&tokens(&["--json"])));
    assert!(!wants_json_catalog(&tokens(&["help", "--", "--json"])));
    assert!(!wants_json_catalog(&tokens(&["--", "--json"])));
}
