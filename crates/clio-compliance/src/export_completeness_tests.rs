// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests export completeness and narrowing-note scenarios for the
//! JSON export writer.
//!
//! ## Owns
//! - Full-bank completeness, filtered incompleteness, and narrowing-note tests
//!
//! ## Does not own
//! - Shared export fixtures (see `export_tests`)
//! - The export writer itself (see `export.rs`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::bundle::{BUNDLE_FORMAT, BundleEnvelope, CompletenessManifest, ExportContentMode};
use crate::export::{ExportFilter, export};
use crate::export_tests::{T, args, ctx, fixture, read};
use clio_store::HygieneStore;
use clio_types::Actor;

fn count(manifest: &CompletenessManifest, label: &str) -> usize {
    manifest
        .counts_by_type
        .iter()
        .find(|(l, _)| l == label)
        .map_or(0, |(_, n)| *n)
}

#[test]
fn t22_01_full_bank_export_is_complete_and_parses() {
    let f = fixture("full");
    let ctx = ctx(&f, Actor::Harness);
    let outcome = export(&args(
        &f,
        &ctx,
        ExportFilter::default(),
        ExportContentMode::DsarPlaintext,
    ))
    .unwrap();
    assert!(outcome.manifest.complete);
    assert!(
        outcome.manifest.omissions.is_empty(),
        "{:?}",
        outcome.manifest.omissions
    );
    assert_eq!(outcome.manifest.bank_ids, vec![f.bank.clone()]);
    assert_eq!(outcome.manifest.generated_at, "2026-09-20T00:00:00Z");
    assert_eq!(count(&outcome.manifest, "items"), 5);
    assert_eq!(count(&outcome.manifest, "triples"), 1);
    assert_eq!(count(&outcome.manifest, "assoc_edges"), 1);
    assert_eq!(count(&outcome.manifest, "persona_stable"), 2);
    assert_eq!(count(&outcome.manifest, "persona_preferences"), 1);
    assert_eq!(count(&outcome.manifest, "tasks"), 1);
    assert_eq!(count(&outcome.manifest, "failures"), 1);
    assert_eq!(count(&outcome.manifest, "beliefs"), 1);
    assert!(count(&outcome.manifest, "tombstones") >= 1);
    let envelope: BundleEnvelope = serde_json::from_str(&read(&f)).unwrap();
    assert_eq!(envelope.format, BUNDLE_FORMAT);
    assert_eq!(envelope.manifest, outcome.manifest);
    assert_eq!(envelope.entities.items.len(), 5);
    assert_eq!(
        envelope.entities.tombstones.len(),
        count(&outcome.manifest, "tombstones")
    );
    let cat = |label: &str| {
        outcome
            .manifest
            .counts_by_category
            .iter()
            .find(|(l, _)| l == label)
            .map_or(0, |(_, n)| *n)
    };
    assert_eq!(cat("schema"), 1);
    assert_eq!(cat("gist"), 1);
    assert_eq!(cat("triple"), 1);
    assert_eq!(cat("persona"), 0);
}

#[test]
fn t22_02_filtered_export_reports_incomplete() {
    let f = fixture("filtered");
    let ctx = ctx(&f, Actor::Harness);
    let filter = ExportFilter {
        category: Some("schema".into()),
        ..ExportFilter::default()
    };
    let outcome = export(&args(&f, &ctx, filter, ExportContentMode::DsarPlaintext)).unwrap();
    assert!(!outcome.manifest.complete);
    assert!(
        !outcome.manifest.omissions.is_empty(),
        "{:?}",
        outcome.manifest.omissions
    );
    assert_eq!(count(&outcome.manifest, "items"), 1);
}

#[test]
fn t22_02b_all_narrowing_notes_are_listed() {
    let f = fixture("narrow");
    let ctx = ctx(&f, Actor::Harness);
    f.store.archive_item(&f.bank, "epi-1", T, "tester").unwrap();
    let filter = ExportFilter {
        time_start: Some(T.into()),
        include_archived: false,
        include_discarded: false,
        ..ExportFilter::default()
    };
    let outcome = export(&args(&f, &ctx, filter, ExportContentMode::DsarPlaintext)).unwrap();
    assert!(!outcome.manifest.complete);
    for note in [
        "time range narrowed items",
        "discarded items excluded",
        "archived items excluded",
        "assoc edges exported unfiltered",
    ] {
        assert!(
            outcome.manifest.omissions.iter().any(|o| o == note),
            "missing: {note}"
        );
    }
    assert_eq!(count(&outcome.manifest, "items"), 4);
}
