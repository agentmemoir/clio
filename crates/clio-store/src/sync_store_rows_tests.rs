// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests of the push-watermark body split
//! in `sync_store_rows`: how a stored watermark body yields its
//! `(stamp, entity_id)` pair.
//!
//! ## Owns
//! - The bare-body case: a body without the separator is the empty-entity
//!   spelling and returns `(stamp, "")`
//! - The compound cases: `stamp<sep>` and `stamp<sep>entity_id` split at the
//!   separator
//! - The empty-watermark sentinel case: the bare `1970-01-01T00:00:00Z`
//!   sentinel body binds the same `(stamp, "")` pair
//!
//! ## Does not own
//! - The feed SQL that binds the split pair (`sync_records`,
//!   `sqlite_sync_history`)
//! - The client-side watermark codec (`clio-sync` `protocol`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the push-watermark body split tests.

use crate::sync_store_rows::{WM_ENTITY_SEP, split_push_watermark};

const STAMP: &str = "2026-01-02T03:04:05Z";
const SENTINEL: &str = "1970-01-01T00:00:00Z";

/// A bare body without the separator is tolerated as the empty-entity
/// spelling: it returns the stamp with an empty entity part.
#[test]
fn bare_body_returns_stamp_with_empty_entity() {
    assert_eq!(
        split_push_watermark(STAMP),
        (STAMP.to_owned(), String::new()),
        "bare body must yield the empty pair"
    );
}

/// A stamp followed by the separator alone carries an empty entity part.
#[test]
fn stamp_with_separator_only_returns_empty_entity() {
    let body = format!("{STAMP}{WM_ENTITY_SEP}");
    assert_eq!(
        split_push_watermark(&body),
        (STAMP.to_owned(), String::new()),
        "trailing separator must yield the empty pair"
    );
}

/// A compound body splits at the separator into the full pair.
#[test]
fn stamp_separator_entity_splits_at_separator() {
    let body = format!("{STAMP}{WM_ENTITY_SEP}item-7");
    assert_eq!(
        split_push_watermark(&body),
        (STAMP.to_owned(), "item-7".to_owned()),
        "compound body must split into the full pair"
    );
}

/// The empty-watermark sentinel is passed as a bare stamp and binds the same
/// empty pair as any other separator-less body.
#[test]
fn empty_watermark_sentinel_binds_empty_entity() {
    assert_eq!(
        split_push_watermark(SENTINEL),
        (SENTINEL.to_owned(), String::new()),
        "sentinel body must yield the empty pair"
    );
}
