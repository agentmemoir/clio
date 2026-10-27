// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the single-file JSON memory export writer:
//! the authorization gate, the payload selection (a whole-bank
//! `memory_export_bundle` or one subject's bundle), and the fail-closed write
//! through the durable destination-custody delivery boundary.
//!
//! A DEK-less `ciphertext_backup` cannot carry readable record content, so its
//! domain records keep only their metadata inventory (`crate::export_mask`).
//!
//! ## Owns
//! - `export` entrypoint and the harness/system authorization gate
//! - `ExportFilter`, `ExportArgs`, `ExportDelivery`, and their defaults
//! - `CompletenessManifest` completeness accounting over a selection
//!
//! ## Does not own
//! - Item/bank collection, masking, and the bundle payload builders
//!   (`crate::export_subject`)
//! - The bundle file contract, canonicalization, and checksums (`crate::bundle`)
//! - The controlled delivery sequence (`crate::export_custody`)
//! - Backend SQL (`clio-store`), MCP argument decoding (`clio-mcp`), and restore
//!
//! ## Boundary
//!
//! Changes to the export entrypoint or its selection inputs should normally
//! require changes here. Changes to how entities are enumerated, masked, or how
//! the bundle bytes are built should not.
//!
//! Keep this module focused on the export entrypoint and its inputs.

use crate::bundle::{CompletenessManifest, ExportContentMode};
use crate::export_custody::{DeliveryRefusal, DeliveryResult, DeliverySpec, deliver_export};
use crate::inventory::{InventoryBackend, InventoryFacts};
use crate::subject_request_events::BindingControls;
use clio_store::{DestinationCustodyStore, LocalEventStore};
use clio_types::{Actor, AmError, ErrorCode, OpContext};
use std::path::Path;

/// Upper bound for bank-wide history reads.
pub(crate) const HISTORY_LIMIT: usize = 10_000;

/// Which bounded history reads returned their full row cap (may be truncated).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Truncation {
    /// Task listing reached [`HISTORY_LIMIT`].
    pub(crate) tasks: bool,
    /// Failure listing reached [`HISTORY_LIMIT`].
    pub(crate) failures: bool,
}

/// True when a bounded history read returned the full row cap.
#[must_use]
pub(crate) fn hit_history_limit(rows: usize) -> bool {
    rows >= HISTORY_LIMIT
}

/// Caller-selectable export narrowing; defaults export the whole bank.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportFilter {
    /// Semantic category filter; `None` or `""` exports all categories.
    pub category: Option<String>,
    /// `created_at` lower bound (inclusive).
    pub time_start: Option<String>,
    /// `created_at` upper bound (exclusive).
    pub time_end: Option<String>,
    /// Include hygiene-archived rows.
    pub include_archived: bool,
    /// Include operationally discarded rows.
    pub include_discarded: bool,
    /// One data subject to select; `None` exports the whole bank.
    pub subject_id: Option<String>,
}

impl Default for ExportFilter {
    /// Everything included: the only fully complete selection.
    fn default() -> Self {
        Self {
            category: None,
            time_start: None,
            time_end: None,
            include_archived: true,
            include_discarded: true,
            subject_id: None,
        }
    }
}

/// The durable delivery boundary for an export.
///
/// Every export carries this group and is written through
/// [`deliver_export`]: the durable event intent, the destination-custody row,
/// and the product staging directory all exist before the destination file is
/// written. The direct write exists only behind the test build; production
/// cannot export without this boundary.
pub struct ExportDelivery<'a> {
    /// Durable local event writer for the intent and outcome.
    pub events: &'a dyn LocalEventStore,
    /// Destination-custody persistence.
    pub custody: &'a dyn DestinationCustodyStore,
    /// The accepted copy controls; a refusal stops before any write.
    pub controls: &'a BindingControls,
    /// The export action id the custody row and event key on.
    pub action_id: &'a str,
    /// The product-controlled directory the bytes are staged in.
    pub staging_dir: &'a Path,
    /// The owner of the destination copy.
    pub custody_owner: &'a str,
    /// The action the destination requires before it is settled.
    pub required_action: &'a str,
}

/// Inputs for the in-process memory export tool.
pub struct ExportArgs<'a> {
    /// Item / triple / assoc persistence boundary.
    pub store: &'a dyn clio_store::Store,
    /// Tombstone / subject-item reads.
    pub compliance: &'a dyn clio_store::ComplianceStore,
    /// Persona document reads.
    pub persona: &'a dyn clio_store::PersonaStore,
    /// Task / failure history reads.
    pub history: &'a dyn clio_store::HistoryStore,
    /// Belief object reads.
    pub belief: &'a dyn clio_store::BeliefStore,
    /// Bank / actor context (actor must be privileged).
    pub ctx: &'a OpContext,
    /// Destination file path.
    pub destination: &'a str,
    /// Selection narrowing; non-default filters mark the manifest incomplete.
    pub filter: ExportFilter,
    /// Content mode of the payload.
    pub content_mode: ExportContentMode,
    /// RFC 3339 UTC timestamp recorded as `generated_at`.
    pub timestamp: &'a str,
    /// One data subject to select, when the export is subject-scoped.
    pub subject_id: Option<&'a str>,
    /// Measured live-store facts; required for a subject-scoped export.
    pub facts: Option<&'a InventoryFacts>,
    /// The store backend; required for a subject-scoped export.
    pub backend: Option<InventoryBackend>,
    /// The durable delivery boundary; required to write.
    pub delivery: Option<ExportDelivery<'a>>,
}

/// Export result.
#[derive(Debug, Clone)]
pub struct ExportOutcome {
    /// Destination path written.
    pub destination: String,
    /// Completeness manifest embedded in the file.
    pub manifest: CompletenessManifest,
    /// Content mode of the payload.
    pub content_mode: ExportContentMode,
}

/// In-process memory export: one bundle file plus its completeness manifest.
///
/// AUTHORISATION: only `harness` or `system` actors may export; agent/user
/// callers get [`ErrorCode::Forbidden`] and no file is created. The payload is
/// built fully in memory. The file is written through the durable custody
/// boundary; an [`ExportDelivery`] is required to write.
///
/// # Errors
/// Authorization, missing bank, storage, delivery refusal, or write failures.
pub fn export(args: &ExportArgs<'_>) -> Result<ExportOutcome, AmError> {
    let bank = args.ctx.bank.as_str();
    if bank.is_empty() {
        return Err(AmError::new(ErrorCode::MissingBank, "bank is required"));
    }
    authorize(args.ctx.actor)?;
    let payload = crate::export_subject::build_payload(args, bank)?;
    write_payload(args, &payload.bytes)?;
    Ok(ExportOutcome {
        destination: args.destination.to_owned(),
        manifest: payload.manifest,
        content_mode: args.content_mode,
    })
}

/// Write the payload through the durable delivery boundary.
fn write_payload(args: &ExportArgs<'_>, bytes: &[u8]) -> Result<(), AmError> {
    let Some(delivery) = &args.delivery else {
        return write_direct(args.destination, bytes);
    };
    let spec = DeliverySpec {
        action_id: delivery.action_id,
        destination: Path::new(args.destination),
        staging_dir: delivery.staging_dir,
        custody_owner: delivery.custody_owner,
        required_action: delivery.required_action,
        now: args.timestamp,
    };
    match deliver_export(
        args.ctx,
        delivery.events,
        delivery.custody,
        delivery.controls,
        &spec,
        bytes,
    ) {
        DeliveryResult::Delivered => Ok(()),
        DeliveryResult::Refused(refusal) => Err(refusal_error(refusal)),
        DeliveryResult::Failed(reason) => Err(AmError::new(
            ErrorCode::Internal,
            format!("export delivery failed: {reason}"),
        )),
    }
}

/// Test-only direct write for in-crate test fixtures; the production build
/// fails closed instead, so no export bypasses the custody boundary.
#[cfg(test)]
fn write_direct(destination: &str, bytes: &[u8]) -> Result<(), AmError> {
    std::fs::write(destination, bytes)
        .map_err(|e| AmError::new(ErrorCode::Internal, format!("bundle write failed: {e}")))
}

/// Production guard: an export without its delivery boundary writes nothing.
#[cfg(not(test))]
fn write_direct(_destination: &str, _bytes: &[u8]) -> Result<(), AmError> {
    Err(AmError::new(
        ErrorCode::Internal,
        "export delivery boundary is required",
    ))
}

/// Map a controlled-delivery refusal onto the export error code.
fn refusal_error(refusal: DeliveryRefusal) -> AmError {
    let (code, message) = match refusal {
        DeliveryRefusal::BindingNotAccepted => (
            ErrorCode::Forbidden,
            "export delivery requires accepted copy controls",
        ),
        DeliveryRefusal::DestinationExists => (
            ErrorCode::InvalidArgument,
            "export destination already exists and is never overwritten",
        ),
        DeliveryRefusal::DestinationNotRegularFile => (
            ErrorCode::InvalidArgument,
            "export destination is not a regular file",
        ),
        DeliveryRefusal::DurableIntentFailed => (
            ErrorCode::InvalidArgument,
            "export delivery could not record its durable event intent",
        ),
    };
    AmError::new(code, message)
}

/// Only harness/system actors carry export privilege.
fn authorize(actor: Actor) -> Result<(), AmError> {
    match actor {
        Actor::Harness | Actor::System => Ok(()),
        Actor::Agent | Actor::User => Err(AmError::new(
            ErrorCode::Forbidden,
            "memory export requires a harness or system actor with compliance privilege",
        )),
    }
}

/// Honest completeness: `true` only when nothing was narrowed or truncated.
/// The knowledge graph follows the exported items, so any item narrowing
/// narrows triples and association edges with it; the collections the filters
/// cannot reach are exported bank-wide and say so.
pub(crate) fn completeness(
    filter: &ExportFilter,
    truncation: Truncation,
    assoc_edges_dropped: usize,
) -> (bool, Vec<String>) {
    let mut omissions = Vec::new();
    if filter.category.as_deref().is_some_and(|c| !c.is_empty()) {
        omissions.push("category filter narrowed items".to_owned());
    }
    if filter.time_start.is_some() || filter.time_end.is_some() {
        omissions.push("time range narrowed items".to_owned());
    }
    if !filter.include_discarded {
        omissions.push("discarded items excluded".to_owned());
    }
    if !filter.include_archived {
        omissions.push("archived items excluded".to_owned());
    }
    if !omissions.is_empty() {
        omissions.push("triples and association edges narrowed to the exported items".to_owned());
        if assoc_edges_dropped > 0 {
            omissions.push(format!(
                "{assoc_edges_dropped} association edges dropped: an endpoint was not exported"
            ));
        }
        omissions.push(
            "the export filters reach only the items collection: persona, task, failure, belief, and tombstone rows are exported bank-wide"
                .to_owned(),
        );
    }
    if truncation.tasks {
        omissions.push(format!("task history truncated at {HISTORY_LIMIT} rows"));
    }
    if truncation.failures {
        omissions.push(format!("failure history truncated at {HISTORY_LIMIT} rows"));
    }
    (omissions.is_empty(), omissions)
}

#[cfg(test)]
#[path = "export_completeness_tests.rs"]
mod export_completeness_tests;

#[cfg(test)]
#[path = "export_context_tests.rs"]
mod export_context_tests;

#[cfg(test)]
#[path = "export_security_tests.rs"]
mod export_security_tests;

#[cfg(test)]
#[path = "export_truncation_tests.rs"]
mod export_truncation_tests;
