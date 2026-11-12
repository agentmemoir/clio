// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the legacy tag data file: every row the sync code needs is
//! present, parsed once, and distinct from the others.
//!
//! ## Owns
//! - Coverage for `legacy_tags`
//!
//! ## Does not own
//! - The v2 domain tags (`clio_store::erase_hash`)
//!
//! ## Boundary
//!
//! Tests here break when the data file loses a row it must keep.

use super::{PLAINTEXT_DIGEST, PLAINTEXT_MUTATION_ID, SYNC_CURSOR, SYNC_WATERMARK, legacy_tag};

#[test]
fn every_required_legacy_tag_is_present_and_distinct() {
    let names = [
        PLAINTEXT_DIGEST,
        PLAINTEXT_MUTATION_ID,
        SYNC_CURSOR,
        SYNC_WATERMARK,
    ];
    let mut seen: Vec<&str> = Vec::new();
    for name in names {
        let tag = legacy_tag(name).expect("legacy tag row");
        assert!(!tag.is_empty(), "{name} has a value");
        assert!(!seen.contains(&tag), "{name} is distinct");
        seen.push(tag);
    }
}

#[test]
fn a_missing_row_is_a_clear_error() {
    let err = legacy_tag("no-such-tag").expect_err("unknown row is refused");
    assert!(err.contains("no-such-tag"), "{err}");
}
