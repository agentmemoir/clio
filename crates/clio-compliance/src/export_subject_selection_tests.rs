// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the subject-selection payload tests: the
//! refused calls that lack measured facts or a store backend, the delivered
//! payload's manifest counts, and every narrowing omission the manifest
//! records.
//!
//! ## Owns
//! - The refusal error codes and messages of `build_subject_selection`
//! - The default-filter manifest's delivered counts and payload bytes
//! - Every narrowing omission the subject manifest records
//!
//! ## Does not own
//! - The payload build itself (`super`)
//! - The fixture store (`inventory_map_fixtures.rs`)
//! - The custody delivery sequence (`export_subject_scenarios_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the subject-selection payload's behavior.

use super::build_subject_selection;
use crate::bundle::ExportContentMode;
use crate::export::{ExportArgs, ExportFilter};
use crate::inventory::{InventoryBackend, InventoryFacts};
use crate::inventory_fixture::{OTHER, SELECTED};
use crate::inventory_map_fixtures::MapFixture;
use clio_types::{Actor, Bank, ErrorCode, OpContext};

/// The time the tests stamp into every manifest.
const NOW: &str = "2026-01-01T00:00:00Z";

/// The context the tests pass: one harness actor over the fixture's bank.
fn ctx(bank: &str) -> OpContext {
    OpContext::new(Bank::new(bank.to_owned()), Actor::Harness)
}

/// The export arguments over the fixture's store views, without facts yet.
fn args<'a>(
    fixture: &'a MapFixture,
    context: &'a OpContext,
    filter: ExportFilter,
    facts: Option<&'a InventoryFacts>,
    backend: Option<InventoryBackend>,
) -> ExportArgs<'a> {
    ExportArgs {
        store: &fixture.store,
        compliance: &fixture.store,
        persona: &fixture.store,
        history: &fixture.store,
        belief: &fixture.store,
        ctx: context,
        destination: "unused: the payload build never touches the destination",
        filter,
        content_mode: ExportContentMode::DsarPlaintext,
        timestamp: NOW,
        subject_id: Some(SELECTED),
        facts,
        backend,
        delivery: None,
    }
}

/// A selection without measured facts is refused with the argument error.
#[test]
fn a_subject_selection_without_measured_facts_is_refused() {
    let fixture = MapFixture::create();
    let context = ctx(&fixture.bank);
    let Err(error) = build_subject_selection(
        &args(
            &fixture,
            &context,
            ExportFilter::default(),
            None,
            Some(InventoryBackend::Sqlite),
        ),
        &fixture.bank,
        SELECTED,
    ) else {
        panic!("missing facts must refuse the selection")
    };
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    assert!(
        error.message.contains("measured live-store facts"),
        "the error names the missing facts"
    );
}

/// A selection without a store backend is refused with the argument error.
#[test]
fn a_subject_selection_without_a_backend_is_refused() {
    let fixture = MapFixture::create();
    let facts = fixture.facts();
    let context = ctx(&fixture.bank);
    let Err(error) = build_subject_selection(
        &args(&fixture, &context, ExportFilter::default(), Some(&facts), None),
        &fixture.bank,
        SELECTED,
    ) else {
        panic!("missing backend must refuse the selection")
    };
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    assert!(
        error.message.contains("store backend"),
        "the error names the missing backend"
    );
}

/// The default-filter selection delivers the subject's own records only, and
/// the manifest counts name the subject mapping's selected count.
#[test]
fn the_default_selection_delivers_only_the_selected_subjects_records() {
    let fixture = MapFixture::create();
    let facts = fixture.facts();
    let context = ctx(&fixture.bank);
    let payload = build_subject_selection(
        &args(&fixture, &context, ExportFilter::default(), Some(&facts), Some(InventoryBackend::Sqlite)),
        &fixture.bank,
        SELECTED,
    )
    .expect("the default selection delivers");
    let items = payload
        .manifest
        .counts_by_type
        .iter()
        .find(|(name, _)| name == "items")
        .expect("the manifest carries the items count");
    assert_eq!(items.1, 5, "the manifest count is the selected mapping's count");
    let text = String::from_utf8(payload.bytes).expect("the payload bytes are text");
    assert!(text.contains(SELECTED), "the payload carries the subject");
    assert!(!text.contains(OTHER), "no other subject's record enters the payload");
}

/// A selection under every narrowing records each narrowing it applied, so
/// the manifest is not complete.
#[test]
fn the_manifest_records_every_narrowing_the_selection_applies() {
    let fixture = MapFixture::create();
    let context = ctx(&fixture.bank);
    let facts = fixture.facts();
    let filter = ExportFilter {
        category: Some("persona".to_owned()),
        time_start: Some("2020-01-01T00:00:00Z".to_owned()),
        time_end: Some("2027-01-01T00:00:00Z".to_owned()),
        include_discarded: false,
        include_archived: false,
        ..ExportFilter::default()
    };
    let payload = build_subject_selection(
        &args(&fixture, &context, filter, Some(&facts), Some(InventoryBackend::Sqlite)),
        &fixture.bank,
        SELECTED,
    )
    .expect("the narrowed selection delivers");
    for note in [
        "category filter narrowed items",
        "time range narrowed items",
        "discarded items excluded",
        "archived items excluded",
    ] {
        assert!(
            payload.manifest.omissions.iter().any(|omission| omission == note),
            "the manifest records the applied narrowing: {note}"
        );
    }
    assert!(!payload.manifest.complete, "a narrowed selection is not complete");
    assert_eq!(
        payload.manifest.filter["subject_id"], SELECTED,
        "the manifest names the selected subject"
    );
}