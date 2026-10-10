// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the manifest-validated, idempotent,
//! admission-gated JSON import of a portable export bundle:
//! checksum-before-write verification, per-entity planning, and the chunked
//! batch commit.
//!
//! ## Owns
//! - `ImportArgs` / `ImportReport` and the `import` entrypoint
//! - Validation ordering: bank/timestamp, format identity, checksums, force reason
//! - Idempotency planning classification (skip / create / overwrite / reject)
//! - Chunked `apply_batch` commit (≤ `MAX_BATCH_OPS` per transaction)
//!
//! ## Does not own
//! - The bundle file contract (`bundle.rs`)
//! - Admission scoring, `belief_observe` gating, or batch executor SQL
//! - MCP argument decoding (`clio-mcp`)
//!
//! ## Boundary
//!
//! Changes to import ordering, gating, or report shape should normally require
//! changes here. Changes to bundle layout or storage SQL should not require
//! changes here.
//!
//! Keep this module focused on import orchestration.

#[path = "import_apply.rs"]
mod import_apply;
#[path = "import_rows.rs"]
mod import_rows;

use clio_store::MAX_BATCH_OPS;
use clio_types::{AmError, ErrorCode};
use serde::{Deserialize, Serialize};

use crate::bundle::{self, ParsedBundle};
use import_apply::Planner;

/// Inputs for the JSON bundle import.
pub struct ImportArgs<'a> {
    /// Memory repository (items, triples, assoc edges, opaque envelopes).
    pub store: &'a dyn clio_store::Store,
    /// Atomic batch commit boundary (staged creates).
    pub batch: &'a dyn clio_store::BatchStore,
    /// Persona repository (stable entries, preferences).
    pub persona: &'a dyn clio_store::PersonaStore,
    /// Task/failure history repository.
    pub history: &'a dyn clio_store::HistoryStore,
    /// Belief repository (identity resolution + trajectory appends).
    pub belief: &'a dyn clio_store::BeliefStore,
    /// Bank / actor context; every entity restores into `ctx.bank`.
    pub ctx: &'a clio_types::OpContext,
    /// Source bundle file path.
    pub source: &'a str,
    /// When true, plan and report without any write.
    pub dry_run: bool,
    /// When true, restore over differing existing rows (requires `reason`).
    pub force: bool,
    /// Required non-empty audit reason when `force` is true.
    pub reason: Option<&'a str>,
    /// Admission policy for gated item creates.
    pub policy: clio_admission::AdmissionPolicy<'a>,
    /// Five-factor signal source for item creates.
    pub signals: &'a dyn clio_admission::gated::SignalSource,
    /// Telemetry sink for admission accept/reject events.
    pub sink: &'a dyn clio_admission::gated::EventSink,
    /// Belief create-path signal source (`belief_observe` trait).
    pub belief_signals: &'a dyn clio_belief::BeliefSignalSource,
    /// Caller-supplied event timestamp.
    pub timestamp: &'a str,
}

/// Idempotency report for one import run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportReport {
    /// Rows that would be (or were) created.
    pub would_create: usize,
    /// Entities skipped because they already exist identically.
    pub would_skip: usize,
    /// Entities rejected by admission or the category gate.
    pub would_reject: usize,
    /// Existing entities overwritten under `force`.
    pub would_overwrite: usize,
    /// Masked rejection reasons (capped at 10).
    pub rejection_samples: Vec<String>,
    /// Whether this run was a dry run.
    pub dry_run: bool,
    /// True iff a live run reached the batch commit.
    pub applied: bool,
    /// Honest run notes (capped).
    pub notes: Vec<String>,
}

/// Manifest-validated, idempotent, admission-gated bundle import.
///
/// Order: read + parse, format identity, checksums before any
/// write, force-reason validation, planning, then commit. Every entity restores
/// into `ctx.bank`; corrupt bundles fail even with `force` or `dry_run`.
///
/// # Errors
/// Read/parse failures, format or checksum mismatches, missing force reason,
/// admission-signal failures, or storage failures.
pub fn import(args: &ImportArgs<'_>) -> Result<ImportReport, AmError> {
    if args.ctx.bank.as_str().is_empty() {
        return Err(AmError::new(ErrorCode::MissingBank, "bank is required"));
    }
    if args.timestamp.is_empty() {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "timestamp is required",
        ));
    }
    let raw = std::fs::read_to_string(args.source).map_err(|e| {
        AmError::new(
            ErrorCode::InvalidArgument,
            format!("bundle read failed: {e}"),
        )
    })?;
    let value: serde_json::Value = serde_json::from_str(&raw).map_err(|e| {
        AmError::new(
            ErrorCode::InvalidArgument,
            format!("bundle JSON invalid: {e}"),
        )
    })?;
    let parsed = bundle::parse_bundle(&value)?;
    verify_checksums(&parsed)?;
    if args.force && args.reason.unwrap_or("").trim().is_empty() {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "force import requires a non-empty reason",
        ));
    }
    let mut planner = Planner::new(args, &parsed.envelope);
    planner.run()?;
    let staged_len = planner.staged.len();
    let applied = if args.dry_run {
        false
    } else {
        for chunk in planner.staged.chunks(MAX_BATCH_OPS) {
            args.batch.apply_batch(chunk)?;
        }
        if staged_len > clio_store::MAX_BATCH_OPS {
            planner.note(
                "batch committed in sequential transactions of up to 50 ops; \
                 only each chunk is atomic, earlier chunks persist on later failure"
                    .to_owned(),
            );
        }
        true
    };
    Ok(ImportReport {
        would_create: planner.create,
        would_skip: planner.skip,
        would_reject: planner.reject,
        would_overwrite: planner.overwrite,
        rejection_samples: planner.samples,
        dry_run: args.dry_run,
        applied,
        notes: planner.notes,
    })
}

/// Verify the completeness manifest before any write.
///
/// Both canonical forms must match the manifest: the typed entity payload and
/// the raw `entities` JSON re-serialized. They are equal for legitimate
/// bundles (`canonical_entities_bytes` canonicalizes through a sorted `Value`)
/// and any payload tampering breaks at least the raw form.
fn verify_checksums(parsed: &ParsedBundle) -> Result<(), AmError> {
    let manifest = &parsed.envelope.manifest;
    if manifest.format_version != parsed.envelope.format_version {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            format!(
                "bundle manifest format_version {} does not match envelope {}",
                manifest.format_version, parsed.envelope.format_version
            ),
        ));
    }
    let typed = bundle::entities_checksum(&parsed.envelope.entities);
    let raw = bundle::sha256_hex_bytes(&bundle::canonical_value_bytes(&parsed.raw_entities));
    if typed != manifest.content_sha256 || raw != manifest.content_sha256 {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            format!(
                "bundle checksum mismatch: manifest {}, canonical {}, raw-file {}",
                manifest.content_sha256, typed, raw
            ),
        ));
    }
    Ok(())
}

#[cfg(test)]
#[path = "import_tests.rs"]
mod import_tests;

#[cfg(test)]
#[path = "import_atomicity_tests.rs"]
mod import_atomicity_tests;

#[cfg(test)]
#[path = "import_context_tests.rs"]
mod import_context_tests;

#[cfg(test)]
#[path = "import_restore_tests.rs"]
mod import_restore_tests;

#[cfg(test)]
#[path = "import_secret_scrub_tests.rs"]
mod import_secret_scrub_tests;
