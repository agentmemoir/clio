// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for unit tests of the export manifest
//! completeness helper: a bounded-history read that returns its full row cap
//! must mark the manifest incomplete with an omission note naming the
//! truncated listing.
//!
//! ## Owns
//! - Direct assertions over `completeness` and `hit_history_limit`
//!
//! ## Does not own
//! - The store-backed export flow (see `export_tests` / `export_security_tests`)
//!
//! ## Boundary
//!
//! Changes to the completeness/truncation rule should normally update tests
//! here. Changes to the export writer itself should not require changes here.
//!
//! Test-only. Never compiled into production builds.

use crate::export::{ExportFilter, HISTORY_LIMIT, Truncation, completeness, hit_history_limit};

#[test]
fn export_truncation_marks_manifest_incomplete() {
    let (complete, omissions) = completeness(
        &ExportFilter::default(),
        Truncation {
            tasks: true,
            failures: false,
        },
    );
    assert!(!complete);
    assert_eq!(
        omissions,
        vec![format!("task history truncated at {HISTORY_LIMIT} rows")]
    );
}

#[test]
fn export_no_truncation_stays_complete() {
    let (complete, omissions) = completeness(
        &ExportFilter::default(),
        Truncation {
            tasks: false,
            failures: false,
        },
    );
    assert!(complete);
    assert!(omissions.is_empty(), "{omissions:?}");
}

#[test]
fn export_truncation_combines_with_filter_notes() {
    let filter = ExportFilter {
        category: Some("schema".into()),
        ..ExportFilter::default()
    };
    let (complete, omissions) = completeness(
        &filter,
        Truncation {
            tasks: false,
            failures: true,
        },
    );
    assert!(!complete);
    assert!(
        omissions
            .iter()
            .any(|o| o == "category filter narrowed items")
    );
    assert!(
        omissions
            .iter()
            .any(|o| o.contains("failure history truncated"))
    );
}

#[test]
fn export_hit_history_limit_detects_cap() {
    assert!(!hit_history_limit(HISTORY_LIMIT - 1));
    assert!(hit_history_limit(HISTORY_LIMIT));
}
