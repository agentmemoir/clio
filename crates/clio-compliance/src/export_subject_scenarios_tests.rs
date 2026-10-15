// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the SQLite subject-export scenario tests: a
//! subject-scoped export delivered through the durable event and
//! destination-custody boundary, the intent- and outcome-failure refusals, the
//! restart reconciliation of open destinations, and the overwrite refusal.
//!
//! ## Owns
//! - The shared `Scratch`, `SubjectExport` driver, accepted controls, and
//!   injected-fault event store reused by the Postgres twin
//! - Every SQLite assertion about a delivered or refused subject export
//!
//! ## Does not own
//! - The export entrypoint and the delivery sequence (other modules)
//! - The fixture store (`inventory_map_fixtures.rs`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::bundle::ExportContentMode;
use crate::export::{ExportArgs, ExportDelivery, ExportFilter, ExportOutcome, export};
use crate::export_custody::reconcile_open_destinations;
use crate::export_tests::{Scratch, staging_entries};
use crate::inventory::{InventoryBackend, InventoryFacts};
use crate::inventory_fixture::{OTHER, SELECTED};
use crate::inventory_map_fixtures::MapFixture;
use crate::subject_request_events::BindingControls;
use crate::test_unique::unique;
use clio_store::{
    BeliefStore, ComplianceStore, DestinationCustodyStore, HistoryStore, LocalEventIntent,
    LocalEventState, LocalEventStore, LocalEventView, PersonaStore, Store,
};
use clio_types::{AmError, Bank, ErrorCode, OpContext};
use std::path::Path;

/// The time the tests pass to every delivery.
pub(crate) const NOW: &str = "2026-01-01T00:00:00Z";

/// Every repository view one subject export reads or writes.
pub(crate) trait ExportViews:
    Store
    + ComplianceStore
    + PersonaStore
    + HistoryStore
    + BeliefStore
    + LocalEventStore
    + DestinationCustodyStore
{
}
impl<
    T: Store
        + ComplianceStore
        + PersonaStore
        + HistoryStore
        + BeliefStore
        + LocalEventStore
        + DestinationCustodyStore,
> ExportViews for T
{
}

/// An injected event-writer fault: an intent failure with no store, or an
/// outcome failure over a real store that records the intent.
pub(crate) struct FaultEvents<'a> {
    inner: Option<&'a dyn LocalEventStore>,
    fail_intent: bool,
}

impl<'a> FaultEvents<'a> {
    /// An intent write that always fails before any effect.
    pub(crate) fn intent_fails() -> FaultEvents<'static> {
        FaultEvents {
            inner: None,
            fail_intent: true,
        }
    }

    /// A real store that records intents while every outcome write fails.
    pub(crate) fn outcome_fails_over(store: &'a dyn LocalEventStore) -> Self {
        Self {
            inner: Some(store),
            fail_intent: false,
        }
    }
}

impl LocalEventStore for FaultEvents<'_> {
    fn record_intent(&self, intent: &LocalEventIntent<'_>) -> Result<(), AmError> {
        if self.fail_intent {
            return Err(AmError::new(
                ErrorCode::Internal,
                "injected intent-write failure",
            ));
        }
        match self.inner {
            Some(inner) => inner.record_intent(intent),
            None => Ok(()),
        }
    }

    fn record_outcome(
        &self,
        _action_id: &str,
        _denied: bool,
        _result: &str,
    ) -> Result<(), AmError> {
        Err(AmError::new(
            ErrorCode::Internal,
            "injected outcome-write failure",
        ))
    }

    fn pending_actions(&self) -> Result<Vec<String>, AmError> {
        match self.inner {
            Some(inner) => inner.pending_actions(),
            None => Ok(Vec::new()),
        }
    }

    fn list_events(&self) -> Result<Vec<LocalEventView>, AmError> {
        match self.inner {
            Some(inner) => inner.list_events(),
            None => Ok(Vec::new()),
        }
    }

    fn prune_completed_before(&self, cutoff: &str) -> Result<usize, AmError> {
        match self.inner {
            Some(inner) => inner.prune_completed_before(cutoff),
            None => Ok(0),
        }
    }
}

/// The controls that accept the copy for a gated delivery.
pub(crate) fn accepted_controls() -> BindingControls {
    BindingControls {
        copy_controls_accepted: true,
        ..BindingControls::default()
    }
}

/// A privileged operation context for one bank.
pub(crate) fn harness_ctx(bank: &str) -> OpContext {
    OpContext::new(Bank::new(bank.to_owned()), clio_types::Actor::Harness)
}

/// One subject export over a store's views and a delivery boundary.
pub(crate) struct SubjectExport<'a> {
    pub(crate) views: &'a dyn ExportViews,
    pub(crate) ctx: &'a OpContext,
    pub(crate) destination: &'a str,
    pub(crate) staging_dir: &'a Path,
    pub(crate) facts: &'a InventoryFacts,
    pub(crate) backend: InventoryBackend,
    pub(crate) controls: &'a BindingControls,
    pub(crate) action_id: &'a str,
}

impl SubjectExport<'_> {
    /// Run the export with `views` as the event writer.
    pub(crate) fn run(&self) -> Result<ExportOutcome, AmError> {
        self.run_with_events(self.views)
    }

    /// Run the export with an explicit event writer and `views` as custody.
    pub(crate) fn run_with_events(
        &self,
        events: &dyn LocalEventStore,
    ) -> Result<ExportOutcome, AmError> {
        export(&ExportArgs {
            store: self.views,
            compliance: self.views,
            persona: self.views,
            history: self.views,
            belief: self.views,
            ctx: self.ctx,
            destination: self.destination,
            filter: ExportFilter {
                subject_id: Some(SELECTED.to_owned()),
                ..ExportFilter::default()
            },
            content_mode: ExportContentMode::DsarPlaintext,
            timestamp: NOW,
            subject_id: Some(SELECTED),
            facts: Some(self.facts),
            backend: Some(self.backend),
            delivery: Some(ExportDelivery {
                events,
                custody: self.views,
                controls: self.controls,
                action_id: self.action_id,
                staging_dir: self.staging_dir,
                custody_owner: "clio",
                required_action: "delivery",
            }),
        })
    }
}

#[test]
fn subject_export_delivers_under_custody_with_only_the_selected_subject() {
    let fixture = MapFixture::create();
    let scratch = Scratch::new();
    let controls = accepted_controls();
    let ctx = harness_ctx(&fixture.bank);
    let action_id = unique("exp-subj");
    let dest_str = scratch.destination().to_string_lossy().into_owned();
    let staging = scratch.staging_dir();
    let facts = fixture.facts();
    let outcome = SubjectExport {
        views: &fixture.store,
        ctx: &ctx,
        destination: &dest_str,
        staging_dir: &staging,
        facts: &facts,
        backend: InventoryBackend::Sqlite,
        controls: &controls,
        action_id: &action_id,
    }
    .run()
    .expect("the subject export delivers");

    assert!(scratch.destination().exists(), "the destination exists");
    let row = fixture
        .store
        .load(&action_id)
        .expect("load")
        .expect("custody row");
    assert_eq!(row.state, "delivered");
    assert_eq!(row.custody_owner, "clio");
    assert_eq!(row.required_action, "delivery");
    assert!(
        staging_entries(&staging).is_empty(),
        "the managed staging file is removed"
    );
    let text = std::fs::read_to_string(scratch.destination()).expect("read the payload");
    assert!(
        !text.contains(OTHER),
        "no other subject marker in the payload"
    );
    let items = outcome
        .manifest
        .counts_by_type
        .iter()
        .find(|(name, _)| name == "items")
        .expect("the manifest carries the items count");
    assert_eq!(
        items.1, 5,
        "the manifest count is the subject mapping's selected count"
    );
}

#[test]
fn subject_export_manifest_records_the_narrowing_it_applies() {
    let fixture = MapFixture::create();
    let scratch = Scratch::new();
    let ctx = harness_ctx(&fixture.bank);
    let dest_str = scratch.destination().to_string_lossy().into_owned();
    let facts = fixture.facts();
    let outcome = export(&ExportArgs {
        store: &fixture.store,
        compliance: &fixture.store,
        persona: &fixture.store,
        history: &fixture.store,
        belief: &fixture.store,
        ctx: &ctx,
        destination: &dest_str,
        filter: ExportFilter {
            subject_id: Some(SELECTED.to_owned()),
            include_discarded: false,
            ..ExportFilter::default()
        },
        content_mode: ExportContentMode::DsarPlaintext,
        timestamp: NOW,
        subject_id: Some(SELECTED),
        facts: Some(&facts),
        backend: Some(InventoryBackend::Sqlite),
        delivery: None,
    })
    .expect("the narrowed subject export delivers");

    assert!(
        outcome
            .manifest
            .omissions
            .iter()
            .any(|note| note == "discarded items excluded"),
        "the omission names the narrowing the selection applied"
    );
    assert!(
        !outcome.manifest.complete,
        "a narrowed selection is not complete"
    );
    let text = std::fs::read_to_string(scratch.destination()).expect("read the payload");
    assert!(
        text.contains("discarded items excluded"),
        "the delivered file carries the same named omission"
    );
    assert!(
        !text.contains(OTHER),
        "no other subject marker in the payload"
    );
}

#[test]
fn subject_export_intent_failure_refuses_with_no_stage_or_destination() {
    let fixture = MapFixture::create();
    let scratch = Scratch::new();
    let controls = accepted_controls();
    let ctx = harness_ctx(&fixture.bank);
    let action_id = unique("exp-intent");
    let dest_str = scratch.destination().to_string_lossy().into_owned();
    let staging = scratch.staging_dir();
    let facts = fixture.facts();
    let err = SubjectExport {
        views: &fixture.store,
        ctx: &ctx,
        destination: &dest_str,
        staging_dir: &staging,
        facts: &facts,
        backend: InventoryBackend::Sqlite,
        controls: &controls,
        action_id: &action_id,
    }
    .run_with_events(&FaultEvents::intent_fails())
    .expect_err("an intent failure refuses");

    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert!(!scratch.destination().exists(), "no destination write");
    assert!(!staging.exists(), "no staging directory was created");
    assert!(fixture.store.load(&action_id).expect("load").is_none());
}

#[test]
fn subject_export_outcome_failure_keeps_one_pending_action_and_one_delivered_row() {
    let mut fixture = MapFixture::create();
    let scratch = Scratch::new();
    let controls = accepted_controls();
    let ctx = harness_ctx(&fixture.bank);
    let action_id = unique("exp-outcome");
    let dest_str = scratch.destination().to_string_lossy().into_owned();
    let staging = scratch.staging_dir();
    let facts = fixture.facts();
    let err = SubjectExport {
        views: &fixture.store,
        ctx: &ctx,
        destination: &dest_str,
        staging_dir: &staging,
        facts: &facts,
        backend: InventoryBackend::Sqlite,
        controls: &controls,
        action_id: &action_id,
    }
    .run_with_events(&FaultEvents::outcome_fails_over(&fixture.store))
    .expect_err("an outcome failure is reported");

    assert_eq!(err.code, ErrorCode::Internal);
    assert!(scratch.destination().exists(), "the file was delivered");
    let row = fixture
        .store
        .load(&action_id)
        .expect("load")
        .expect("custody row");
    assert_eq!(
        row.state, "delivered",
        "the delivered copy is recorded once"
    );
    assert_eq!(
        fixture
            .store
            .pending_actions()
            .expect("pending")
            .iter()
            .filter(|id| **id == action_id)
            .count(),
        1,
        "exactly one pending action remains for the export"
    );
    // A restart reopens the same file under a fresh handle, and the reconcile
    // completes that action once and never writes a second bundle: the
    // delivered row is settled, not open.
    let before = std::fs::read(scratch.destination()).expect("read the payload");
    fixture.restart();
    let open = reconcile_open_destinations(&fixture.store, &fixture.store).expect("reconcile");
    assert!(
        open.is_empty(),
        "the delivered row is not an open destination"
    );
    assert!(
        !fixture
            .store
            .pending_actions()
            .expect("pending")
            .contains(&action_id),
        "the one pending action is completed exactly once"
    );
    let recorded: Vec<_> = fixture
        .store
        .list_events()
        .expect("events")
        .into_iter()
        .filter(|event| event.action_id == action_id)
        .collect();
    assert_eq!(recorded.len(), 1, "exactly one event row for the action");
    assert_eq!(recorded[0].state, LocalEventState::Allowed);
    assert_eq!(
        std::fs::read(scratch.destination()).expect("read the payload"),
        before,
        "reconciliation writes no second bundle"
    );
}
