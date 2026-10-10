// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `verify` operator surface: deeper
//! pass/fail consistency checks over referential integrity and index coverage.
//!
//! ## Owns
//! - [`verify`]: integrity signal checks, vector/lexical coverage checks, the
//!   manifest-availability note, and the aggregate `ok` bit
//! - [`VerifyReport`] shape
//!
//! ## Does not own
//! - The diagnose snapshot (`diagnose`)
//! - Repair planning or execution (`doctor` / `repair`)
//!
//! ## Boundary
//!
//! Changes to the consistency check set should normally require changes here.
//! Changes to the diagnose snapshot should not require changes here.
//!
//! Findings carry counts and labels only; verify never returns memory content.
//! Severity policy: integrity and coverage failures are `error` (aggregate
//! `ok=false`, CLI exit non-zero); derived-structure and manifest notes are
//! `warning`/`info` and never fail the aggregate.

use crate::finding::{Findings, OpsFinding, Severity, codes, scope_label};
use clio_store::{IndexStore, IntegritySignals, OpsStore};
use clio_types::AmError;
use serde::Serialize;

/// Inputs for [`verify`].
pub struct VerifyArgs<'a> {
    /// Operator health repository.
    pub store: &'a dyn OpsStore,
    /// Index backend view (coverage counts).
    pub index: &'a dyn IndexStore,
    /// Bank scope (`None` = whole store).
    pub bank: Option<&'a str>,
    /// Caller-supplied timestamp (determinism).
    pub timestamp: &'a str,
}

/// Deeper consistency report (PII-safe; counts and finding codes only).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct VerifyReport {
    /// True when no `Error` finding exists.
    pub ok: bool,
    /// Report timestamp.
    pub timestamp: String,
    /// Scope (`store` or `bank:<id>`).
    pub scope: String,
    /// Referential-integrity signal counts for the scope.
    pub integrity: IntegritySignals,
    /// Open, non-discarded indexable items in scope.
    pub indexable_items: u64,
    /// Dense vector rows in scope.
    pub dense_rows: u64,
    /// Lexical rows in scope.
    pub lexical_rows: u64,
    /// Findings in deterministic order.
    pub findings: Vec<OpsFinding>,
}

/// Run the consistency pass.
///
/// # Errors
/// Storage/index read failures (an unreadable store cannot be verified).
pub fn verify(args: &VerifyArgs<'_>) -> Result<VerifyReport, AmError> {
    let scope = scope_label(args.bank);
    let mut findings = Findings::new();
    let integrity = args.store.integrity_signals(args.bank)?;
    let coverage = args.index.index_coverage(args.bank)?;

    // Referential integrity: an orphan is always an error-severity finding.
    if integrity.orphan_assoc_edges > 0 {
        findings.push(OpsFinding::new(
            codes::ORPHAN_ASSOC_EDGE,
            Severity::Error,
            &scope,
            format!(
                "{} association edge(s) reference a missing item",
                integrity.orphan_assoc_edges
            ),
        ));
    }
    if integrity.orphan_triple_items > 0 {
        findings.push(OpsFinding::new(
            codes::DANGLING_TRIPLE_REF,
            Severity::Error,
            &scope,
            format!(
                "{} triple row(s) reference a missing carrier item",
                integrity.orphan_triple_items
            ),
        ));
    }
    if integrity.dangling_snapshot_refs > 0 {
        findings.push(OpsFinding::new(
            codes::DANGLING_SNAPSHOT_REF,
            Severity::Error,
            &scope,
            format!(
                "{} triple snapshot_ref value(s) reference a missing item",
                integrity.dangling_snapshot_refs
            ),
        ));
    }
    if integrity.dangling_gist_refs > 0 {
        findings.push(OpsFinding::new(
            codes::DANGLING_GIST_REF,
            Severity::Error,
            &scope,
            format!(
                "{} triple gist_ref value(s) reference a missing item",
                integrity.dangling_gist_refs
            ),
        ));
    }
    if integrity.orphan_memtree_leaf_items > 0 {
        findings.push(OpsFinding::new(
            codes::ORPHAN_MEMTREE_LEAF,
            Severity::Warning,
            &scope,
            format!(
                "{} MemTree leaf node(s) reference a missing item",
                integrity.orphan_memtree_leaf_items
            ),
        ));
    }

    // Vector coverage vs indexable leaves.
    if coverage.dense_rows < coverage.indexable_items {
        findings.push(OpsFinding::new(
            codes::DENSE_COVERAGE_GAP,
            Severity::Error,
            &scope,
            format!(
                "{} dense row(s) for {} indexable item(s)",
                coverage.dense_rows, coverage.indexable_items
            ),
        ));
    }
    if coverage.lexical_rows < coverage.indexable_items {
        findings.push(OpsFinding::new(
            codes::LEXICAL_COVERAGE_GAP,
            Severity::Error,
            &scope,
            format!(
                "{} lexical row(s) for {} indexable item(s)",
                coverage.lexical_rows, coverage.indexable_items
            ),
        ));
    }

    // Manifest-vs-store drift needs a persisted export manifest; none exists
    // yet, so the check is skipped and the gap is made explicit (never silent).
    findings.push(OpsFinding::new(
        codes::MANIFEST_UNAVAILABLE,
        Severity::Info,
        &scope,
        "no persisted export manifest; manifest-vs-store drift check skipped".to_owned(),
    ));

    Ok(VerifyReport {
        ok: findings.ok(),
        timestamp: args.timestamp.to_owned(),
        scope,
        integrity,
        indexable_items: coverage.indexable_items,
        dense_rows: coverage.dense_rows,
        lexical_rows: coverage.lexical_rows,
        findings: findings.into_vec(),
    })
}

/// Error-severity finding codes (the aggregate-failing set), for callers that
/// need to decide CI exit status or targeted re-verification.
#[must_use]
pub fn error_codes(findings: &[OpsFinding]) -> Vec<String> {
    findings
        .iter()
        .filter(|f| f.severity == Severity::Error)
        .map(|f| f.code.clone())
        .collect()
}

#[cfg(test)]
#[path = "verify_tests.rs"]
mod verify_tests;
