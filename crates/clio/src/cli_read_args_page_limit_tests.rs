// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the shared page-limit contract the CLI applies to every
//! `--limit` it serves: the documented default, the refusal of a non-positive
//! value, and the clamp of anything above the maximum.
//!
//! ## Owns
//! - `page_limit`'s default, passthrough, clamp, and zero-refusal behaviors
//!
//! ## Does not own
//! - The clamp disclosure in a command's output (`cli_read`)
//! - The required-flag and parser-spec checks (`cli_read_args_tests`,
//!   `cli_read_args_spec_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the shared page-limit contract.

use super::{DEFAULT_PAGE_LIMIT, MAX_PAGE_LIMIT, page_limit};

/// The contract: omitted reads as the documented default, an in-cap value is
/// served as typed, the maximum itself passes, and anything above the maximum
/// is clamped to the maximum rather than refused.
#[test]
fn page_limit_defaults_passes_through_and_clamps_to_the_maximum() {
    assert_eq!(page_limit("limit", None).unwrap(), DEFAULT_PAGE_LIMIT);
    assert_eq!(page_limit("limit", Some(1)).unwrap(), 1);
    assert_eq!(
        page_limit("limit", Some(MAX_PAGE_LIMIT)).unwrap(),
        MAX_PAGE_LIMIT
    );
    // One past the cap and far past it both serve the cap.
    assert_eq!(
        page_limit("limit", Some(MAX_PAGE_LIMIT + 1)).unwrap(),
        MAX_PAGE_LIMIT
    );
    assert_eq!(page_limit("limit", Some(500)).unwrap(), MAX_PAGE_LIMIT);
}

/// `0` is refused as a usage error that names the flag, matching every other
/// caller-supplied flag mistake at this layer.
#[test]
fn page_limit_refuses_a_non_positive_value() {
    let error = page_limit("limit", Some(0)).expect_err("zero is not a page size");
    assert_eq!(error.code(), "usage");
    assert_eq!(
        error.message(),
        "--limit must be a positive page size, got `0`"
    );
    assert_eq!(error.hint(), Some("try `--limit 10`"));
}
