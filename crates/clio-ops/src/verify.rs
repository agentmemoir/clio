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
//!   manifest-availability note, the store-wide note, and the aggregate `ok` bit
//! - [`verify_scoped`]: the same pass over one named bank, which reads no other
//!   bank and so builds no store-wide note
//! - [`store_wide_note`]: the error codes other banks carry, for a reader of a
//!   bank-scoped report
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
//!
//! Scoping policy: a pass for a named bank also reports, in `notes`, the error
//! codes other banks carry, and it is not `ok` while one exists, so a clean
//! bank-scoped report is never read as a clean store. The scoped form a caller
//! whose grant covers one bank uses never runs that probe.

use crate::finding::{Findings, OpsFinding, Severity, codes, scope_label};
use clio_store::{IndexCoverage, IndexStore, IntegritySignals, OpsStore};
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
    /// True when no `Error` finding exists in scope and no error finding exists
    /// outside the named bank.
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
    /// Notes for skipped checks and findings outside the named bank.
    pub notes: Vec<String>,
}

/// Run the consistency pass over the bank `args.bank` names, or the whole store
/// when it names none.
///
/// A pass for a named bank also reports the error codes other banks carry, and
/// is not `ok` while one exists. A caller whose grant covers one bank must call
/// [`verify_scoped`] instead, which never runs that probe.
///
/// # Errors
/// Storage/index read failures (an unreadable store cannot be verified).
pub fn verify(args: &VerifyArgs<'_>) -> Result<VerifyReport, AmError> {
    verify_inner(args, args.bank, true)
}

/// Run the consistency pass over exactly `bank`, reading no other bank.
///
/// The store-wide note probe never runs here: a caller limited to one bank must
/// not learn that another bank exists or which codes it carries. `bank` is the
/// report's scope whatever `args.bank` names. A caller with no bank to name uses
/// [`verify`].
///
/// # Errors
/// Storage/index read failures (an unreadable store cannot be verified).
pub fn verify_scoped(args: &VerifyArgs<'_>, bank: &str) -> Result<VerifyReport, AmError> {
    verify_inner(args, Some(bank), false)
}

/// The probe-free pass a repair plan is composed from (see [`crate::doctor`]).
///
/// It never builds the store-wide note, so a caller that reports that note
/// itself carries it exactly once.
pub(crate) fn verify_for_plan(
    args: &VerifyArgs<'_>,
    bank: Option<&str>,
) -> Result<VerifyReport, AmError> {
    verify_inner(args, bank, false)
}

/// The report body every entry point shares (`bank` `None` = whole store).
fn verify_inner(
    args: &VerifyArgs<'_>,
    bank: Option<&str>,
    store_wide_notes: bool,
) -> Result<VerifyReport, AmError> {
    let scope = scope_label(bank);
    let mut findings = Findings::new();
    let integrity = args.store.integrity_signals(bank)?;
    let coverage = args.index.index_coverage(bank)?;
    push_integrity_findings(&integrity, &scope, &mut findings);
    push_coverage_findings(&coverage, &scope, &mut findings);

    // Manifest-vs-store drift needs a persisted export manifest; none exists
    // yet, so the check is skipped and the gap is made explicit (never silent).
    findings.push(OpsFinding::new(
        codes::MANIFEST_UNAVAILABLE,
        Severity::Info,
        &scope,
        "no persisted export manifest; manifest-vs-store drift check skipped".to_owned(),
    ));

    let note = bank
        .filter(|_| store_wide_notes)
        .and_then(|bank| store_wide_note(args.store, args.index, bank, args.timestamp));

    Ok(VerifyReport {
        ok: findings.ok() && note.is_none(),
        timestamp: args.timestamp.to_owned(),
        scope,
        integrity,
        indexable_items: coverage.indexable_items,
        dense_rows: coverage.dense_rows,
        lexical_rows: coverage.lexical_rows,
        findings: findings.into_vec(),
        notes: note.into_iter().collect(),
    })
}

/// Referential integrity: an orphan association edge or a dangling triple
/// reference is an `Error` finding; a dangling `MemTree` leaf only a `Warning`.
fn push_integrity_findings(integrity: &IntegritySignals, scope: &str, findings: &mut Findings) {
    if integrity.orphan_assoc_edges > 0 {
        findings.push(OpsFinding::new(
            codes::ORPHAN_ASSOC_EDGE,
            Severity::Error,
            scope,
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
            scope,
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
            scope,
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
            scope,
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
            scope,
            format!(
                "{} MemTree leaf node(s) reference a missing item",
                integrity.orphan_memtree_leaf_items
            ),
        ));
    }
}

/// Derived-index coverage against indexable leaves: fewer rows than items is an
/// `Error` finding, per index.
fn push_coverage_findings(coverage: &IndexCoverage, scope: &str, findings: &mut Findings) {
    if coverage.dense_rows < coverage.indexable_items {
        findings.push(OpsFinding::new(
            codes::DENSE_COVERAGE_GAP,
            Severity::Error,
            scope,
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
            scope,
            format!(
                "{} lexical row(s) for {} indexable item(s)",
                coverage.lexical_rows, coverage.indexable_items
            ),
        ));
    }
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

/// A bank-scoped pass sees only that bank's findings. Verify every other known
/// bank and name any error code it carries, so a reader of a bank-scoped report
/// does not read a clean report as a clean store. Comparing codes across banks
/// (not against the whole-store aggregate) catches a gap in another bank even
/// when the scoped bank carries the same code. A store whose bank list cannot be
/// read adds no claim.
#[must_use]
pub fn store_wide_note(
    store: &dyn OpsStore,
    index: &dyn IndexStore,
    bank: &str,
    timestamp: &str,
) -> Option<String> {
    let mut outside: Vec<String> = Vec::new();
    for other in store.bank_ids().ok()? {
        if other == bank {
            continue;
        }
        let Ok(report) = verify_scoped(
            &VerifyArgs {
                store,
                index,
                bank: Some(other.as_str()),
                timestamp,
            },
            other.as_str(),
        ) else {
            continue;
        };
        for code in error_codes(&report.findings) {
            if !outside.contains(&code) {
                outside.push(code);
            }
        }
    }
    if outside.is_empty() {
        return None;
    }
    outside.sort();
    Some(format!(
        "store-wide findings outside bank `{bank}`: {}; run `clio ops verify` without --bank",
        outside.join(", ")
    ))
}

#[cfg(test)]
#[path = "verify_tests.rs"]
mod verify_tests;
