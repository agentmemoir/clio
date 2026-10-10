// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the crate-root contracts: the serve TTL and
//! the pull page bound this crate exposes as constants.
//!
//! ## Owns
//! - Behavior coverage for the crate root's public constants
//!
//! ## Does not own
//! - The surfaces those constants feed (`server`, `client`)
//! - The help text that documents the TTL (`clio`)
//!
//! ## Boundary
//!
//! Tests here break when a crate-root constant changes; they never change it.
//!
//! Test-only. Never compiled into production builds.

use super::{PULL_PAGE_LIMIT, SERVE_TTL_DEFAULT_SECS};

/// The interactive serve TTL stays at the documented default of 30 minutes:
/// the usage text promises it, so code and help cannot drift apart.
#[test]
fn the_serve_ttl_matches_its_documented_default() {
    assert_eq!(SERVE_TTL_DEFAULT_SECS, 30 * 60);
}

/// Pull pages stay bounded, and at the documented page size: sync must never
/// request an unbounded result set.
///
/// The bound is pinned by value rather than by a second `assert!(LIMIT > 0)`.
/// That form compares two constants, so the check is settled while the crate is
/// compiled rather than while the test runs, which is why the linter rejects it
/// under `-D warnings`. Pinning the value is the form the linter accepts, and
/// its failure message names the bound that moved.
#[test]
fn the_pull_page_stays_bounded() {
    assert_eq!(
        PULL_PAGE_LIMIT, 200,
        "the pull page must stay bounded at its documented size"
    );
}
