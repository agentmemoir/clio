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

#[test]
fn a_line_without_a_tab_separator_is_refused() {
    let err = super::parse("plaintext_digest no-tab\n").expect_err("no separator is refused");
    assert!(err.contains("line 1"), "{err}");
}

#[test]
fn an_empty_field_is_refused() {
    let no_value = super::parse("plaintext_digest\t\n").expect_err("empty value is refused");
    assert!(no_value.contains("line 1"), "{no_value}");
    let no_name = super::parse("\tvalue\n").expect_err("empty name is refused");
    assert!(no_name.contains("line 1"), "{no_name}");
}

#[test]
fn a_file_with_no_rows_is_refused() {
    let err = super::parse("# only a comment\n\n").expect_err("no rows is refused");
    assert!(err.contains("no rows"), "{err}");
}

#[test]
fn blank_and_comment_lines_and_carriage_returns_are_skipped() {
    let rows = super::parse("# comment\r\n\r\nplaintext_digest\tvalue\r\n").expect("rows");
    assert_eq!(rows, vec![("plaintext_digest", "value")]);
}
