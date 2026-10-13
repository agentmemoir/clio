// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `diagnose` operator surface: a fast
//! read-only install/database health snapshot with stable, automation-friendly
//! field names and PII-free findings.
//!
//! ## Owns
//! - [`diagnose`]: backend ping, schema version, bank list, vector index
//!   presence, embedding readiness, coverage counts, and the `ok` bit
//! - [`DiagnoseReport`] shape
//!
//! ## Does not own
//! - Consistency checks (`verify`)
//! - Repair planning or execution (`doctor` / `repair`)
//!
//! ## Boundary
//!
//! Changes to the diagnose probe set or report shape should normally require
//! changes here. Changes to deeper integrity checks should not require changes
//! here.
//!
//! Every probe failure is reported as a finding; diagnose never panics on an
//! unreachable backend and never echoes memory content.

use crate::finding::{Findings, OpsFinding, Severity, codes, scope_label};
use clio_index::Embedder;
use clio_store::{IndexStore, OpsStore};
use serde::Serialize;

/// The command that checks the index-coverage and referential-integrity
/// questions `diagnose` deliberately leaves out, so a healthy snapshot points a
/// reader at the deeper pass instead of standing in for it.
pub const VERIFY_POINTER: &str = "clio ops verify";

/// Inputs for [`diagnose`].
pub struct DiagnoseArgs<'a> {
    /// Operator health repository.
    pub store: &'a dyn OpsStore,
    /// Index backend view (coverage counts).
    pub index: &'a dyn IndexStore,
    /// Optional embedding sidecar probe.
    pub embedder: Option<&'a dyn Embedder>,
    /// Caller-supplied timestamp (determinism).
    pub timestamp: &'a str,
}

/// Read-only install/database health snapshot (PII-safe).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DiagnoseReport {
    /// True when no `Error` finding exists.
    pub ok: bool,
    /// Snapshot timestamp.
    pub timestamp: String,
    /// Backend ping succeeded.
    pub backend_reachable: bool,
    /// Recorded schema version, if readable.
    pub schema_version: Option<String>,
    /// Schema version this build expects.
    pub expected_schema_version: String,
    /// Distinct banks seen in the item table.
    pub banks: Vec<String>,
    /// Derived vector/lexical index state was readable.
    pub vector_index_present: bool,
    /// Active embedding model id recorded by the store, if readable.
    pub embedding_model: Option<String>,
    /// Active embedding width recorded by the store, if readable.
    pub embedding_dims: Option<usize>,
    /// Index mode serving the active width (`ann_vector`/`ann_halfvec`/`exact_scan`).
    pub index_mode: Option<String>,
    /// Open, non-discarded indexable items (whole store).
    pub indexable_items: u64,
    /// Dense vector rows (whole store).
    pub dense_rows: u64,
    /// Lexical rows (whole store).
    pub lexical_rows: u64,
    /// Pending index jobs (whole store).
    pub pending_jobs: u64,
    /// Failed index jobs awaiting retry (whole store).
    pub retry_jobs: u64,
    /// Embedding sidecar ready, when a probe was supplied.
    pub embedding_ready: Option<bool>,
    /// Findings in deterministic order.
    pub findings: Vec<OpsFinding>,
    /// Deeper index-coverage and referential-integrity checks live here rather
    /// than in this snapshot, so an `ok` verdict names what it does not cover.
    pub verify_pointer: &'static str,
}

/// Run the read-only health snapshot. Never fails: probe errors become findings.
#[must_use]
pub fn diagnose(args: &DiagnoseArgs<'_>) -> DiagnoseReport {
    let mut findings = Findings::new();
    let reachable = match args.store.ping() {
        Ok(()) => true,
        Err(e) => {
            findings.push(OpsFinding::new(
                codes::BACKEND_UNREACHABLE,
                Severity::Error,
                scope_label(None),
                format!("backend ping failed: {}", sanitize(&e.to_string())),
            ));
            false
        }
    };

    let schema_version = read_schema_version(args.store, &mut findings);

    let banks = match args.store.bank_ids() {
        Ok(banks) => banks,
        Err(e) => {
            findings.push(OpsFinding::new(
                codes::SYSTEM_TABLES_UNREADABLE,
                Severity::Error,
                scope_label(None),
                format!("bank inventory unreadable: {}", sanitize(&e.to_string())),
            ));
            Vec::new()
        }
    };

    let (vector_index_present, indexable_items, dense_rows, lexical_rows, pending, retry) =
        match args.index.index_coverage(None) {
            Ok(cov) => (
                true,
                cov.indexable_items,
                cov.dense_rows,
                cov.lexical_rows,
                cov.pending_jobs,
                cov.retry_jobs,
            ),
            Err(e) => {
                findings.push(OpsFinding::new(
                    codes::VECTOR_INDEX_MISSING,
                    Severity::Error,
                    scope_label(None),
                    format!(
                        "derived index state unreadable: {}",
                        sanitize(&e.to_string())
                    ),
                ));
                (false, 0, 0, 0, 0, 0)
            }
        };

    // Active embedding space: the operator needs to see which model/width the
    // derived dense index was built for, and whether an ANN index serves it.
    let (embedding_model, embedding_dims, index_mode) =
        read_embedding_space(args.store, &mut findings);

    // Corruption-signal probe: integrity counts are not included in the
    // snapshot, but an unreadable signal table is itself a corruption signal.
    if let Err(e) = args.store.integrity_signals(None) {
        findings.push(OpsFinding::new(
            codes::SYSTEM_TABLES_UNREADABLE,
            Severity::Error,
            scope_label(None),
            format!("integrity tables unreadable: {}", sanitize(&e.to_string())),
        ));
    }

    let embedding_ready = args.embedder.map(|embedder| match embedder.health() {
        Ok(()) => true,
        Err(e) => {
            findings.push(OpsFinding::new(
                codes::EMBEDDING_UNREACHABLE,
                Severity::Warning,
                scope_label(None),
                format!(
                    "embedding sidecar health failed: {}",
                    sanitize(&e.to_string())
                ),
            ));
            false
        }
    });

    DiagnoseReport {
        ok: findings.ok(),
        timestamp: args.timestamp.to_owned(),
        backend_reachable: reachable,
        schema_version,
        expected_schema_version: clio_store::SCHEMA_VERSION.to_owned(),
        banks,
        vector_index_present,
        embedding_model,
        embedding_dims,
        index_mode,
        indexable_items,
        dense_rows,
        lexical_rows,
        pending_jobs: pending,
        retry_jobs: retry,
        embedding_ready,
        findings: findings.into_vec(),
        verify_pointer: VERIFY_POINTER,
    }
}

/// Probe the active embedding space, pushing an unreadable-space finding.
fn read_embedding_space(
    store: &dyn OpsStore,
    findings: &mut Findings,
) -> (Option<String>, Option<usize>, Option<String>) {
    match store.embedding_space() {
        Ok(space) => (
            Some(space.model_id().to_owned()),
            Some(space.dims()),
            Some(space.index_mode().as_str().to_owned()),
        ),
        Err(e) => {
            findings.push(OpsFinding::new(
                codes::SYSTEM_TABLES_UNREADABLE,
                Severity::Error,
                scope_label(None),
                format!("embedding space unreadable: {}", sanitize(&e.to_string())),
            ));
            (None, None, None)
        }
    }
}

/// Read the recorded schema version, pushing readability/mismatch findings.
fn read_schema_version(store: &dyn OpsStore, findings: &mut Findings) -> Option<String> {
    let version = match store.schema_version() {
        Ok(v) => v,
        Err(e) => {
            findings.push(OpsFinding::new(
                codes::SYSTEM_TABLES_UNREADABLE,
                Severity::Error,
                scope_label(None),
                format!("schema_settings unreadable: {}", sanitize(&e.to_string())),
            ));
            return None;
        }
    };
    match version.as_deref() {
        None => findings.push(OpsFinding::new(
            codes::SCHEMA_VERSION_MISSING,
            Severity::Error,
            scope_label(None),
            "schema_version row is missing".to_owned(),
        )),
        Some(v) if v != clio_store::SCHEMA_VERSION => findings.push(OpsFinding::new(
            codes::SCHEMA_VERSION_MISMATCH,
            Severity::Error,
            scope_label(None),
            format!(
                "recorded schema version {v} differs from expected {}",
                clio_store::SCHEMA_VERSION
            ),
        )),
        Some(_) => {}
    }
    version
}

/// Collapse newlines and cap length so driver error text cannot smuggle
/// multi-line payloads into operator logs.
fn sanitize(message: &str) -> String {
    let flat: String = message
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    flat.chars().take(200).collect()
}

#[cfg(test)]
#[path = "diagnose_tests.rs"]
mod diagnose_tests;
