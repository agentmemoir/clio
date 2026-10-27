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
use crate::export_tests::{T, args, ctx, fixture, item, read, triple};
use clio_store::{AssocEdgeRecord, AssocKind, HygieneStore, Store};
use clio_types::{Actor, EpisodicType};
use std::collections::BTreeSet;

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
        "triples and association edges narrowed to the exported items",
        "1 association edges dropped: an endpoint was not exported",
    ] {
        assert!(
            outcome.manifest.omissions.iter().any(|o| o == note),
            "missing: {note}"
        );
    }
    assert_eq!(count(&outcome.manifest, "items"), 4);
    assert_eq!(count(&outcome.manifest, "assoc_edges"), 0);
}

#[test]
fn t22_02c_category_narrowing_narrows_the_graph() {
    let f = fixture("graph-category");
    let ctx = ctx(&f, Actor::Harness);
    let filter = ExportFilter {
        category: Some("schema".into()),
        ..ExportFilter::default()
    };
    let outcome = export(&args(&f, &ctx, filter, ExportContentMode::DsarPlaintext)).unwrap();
    assert_eq!(count(&outcome.manifest, "items"), 1);
    assert_eq!(count(&outcome.manifest, "triples"), 0);
    assert_eq!(count(&outcome.manifest, "assoc_edges"), 0);
    assert!(!outcome.manifest.complete);
    assert!(
        outcome.manifest.omissions.iter().any(|o| {
            o.contains("the export filters reach only the items collection")
                && o.contains("persona")
        }),
        "{:?}",
        outcome.manifest.omissions
    );
}

#[test]
fn t22_02d_time_start_drops_the_graph_rows_whose_items_predate_it() {
    let f = fixture("graph-time");
    let late = "2026-07-01T00:00:00Z";
    let mut carrier = item(
        "tri-late",
        &f.bank,
        "sub-c",
        None,
        Some(EpisodicType::Triple),
        None,
    );
    carrier.created_at = late.into();
    let mut late_triple = triple(&f.bank);
    late_triple.id = "triple-late".into();
    late_triple.item_id = "tri-late".into();
    late_triple.predicate = "prefers".into();
    late_triple.created_at = late.into();
    f.store
        .commit_triple_add(&carrier, &late_triple, false, "tester")
        .unwrap();
    f.store
        .put_assoc_edge(&AssocEdgeRecord {
            id: "edge-late".into(),
            bank_id: f.bank.clone(),
            source_id: "tri-late".into(),
            target_id: "tri-late".into(),
            assoc_kind: AssocKind::Explicit,
            relationship: None,
            weight: 0.5,
            last_updated_at: late.into(),
        })
        .unwrap();
    let ctx = ctx(&f, Actor::Harness);
    let filter = ExportFilter {
        time_start: Some(late.into()),
        ..ExportFilter::default()
    };
    let outcome = export(&args(&f, &ctx, filter, ExportContentMode::DsarPlaintext)).unwrap();
    assert_eq!(count(&outcome.manifest, "items"), 1);
    assert_eq!(count(&outcome.manifest, "triples"), 1);
    assert_eq!(count(&outcome.manifest, "assoc_edges"), 1);
    let envelope: BundleEnvelope = serde_json::from_str(&read(&f)).unwrap();
    assert_eq!(envelope.entities.triples[0].id, "triple-late");
    assert_eq!(envelope.entities.assoc_edges[0].id, "edge-late");
    assert_eq!(envelope.entities.items.len(), 1);
}

#[test]
fn t22_02e_no_exported_edge_names_an_omitted_item() {
    let f = fixture("graph-endpoints");
    let ctx = ctx(&f, Actor::Harness);
    let filter = ExportFilter {
        category: Some("gist".into()),
        ..ExportFilter::default()
    };
    export(&args(&f, &ctx, filter, ExportContentMode::DsarPlaintext)).unwrap();
    let envelope: BundleEnvelope = serde_json::from_str(&read(&f)).unwrap();
    let emitted: BTreeSet<&str> = envelope
        .entities
        .items
        .iter()
        .map(|item| item.id.as_str())
        .collect();
    for edge in &envelope.entities.assoc_edges {
        assert!(
            emitted.contains(edge.source_id.as_str()) && emitted.contains(edge.target_id.as_str()),
            "edge {} names an item the bundle omits",
            edge.id
        );
    }
    for triple in &envelope.entities.triples {
        assert!(
            emitted.contains(triple.item_id.as_str()),
            "triple {} names an item the bundle omits",
            triple.id
        );
    }
}
