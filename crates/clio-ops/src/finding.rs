// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the PII-safe operator finding shape and the
//! stable finding-code catalog shared by diagnose, verify, doctor, and repair.
//!
//! ## Owns
//! - `Severity`, `OpsFinding`, and `Findings` aggregation (the `ok` bit)
//! - The finding-code string catalog
//! - The shared credential redactor applied to every error-derived ops text
//!
//! ## Does not own
//! - Which checks emit which code (diagnose / verify)
//! - Repair action allowlists or plan composition (doctor / repair)
//!
//! ## Boundary
//!
//! Changes to the finding shape or code catalog should normally require changes
//! here. Changes to check logic should not require changes here.
//!
//! A finding carries ids, counts, and labels only — never memory content, DEKs,
//! API keys, or item snapshots.

use serde::{Deserialize, Serialize};

/// Finding severity. `ok` aggregates to false when any `Error` finding exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Informational; never fails the aggregate.
    Info,
    /// Advisory; never fails the aggregate but is worth surfacing.
    Warning,
    /// Consistency failure; fails the aggregate.
    Error,
}

/// One operator finding: `{code, severity, scope, message}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpsFinding {
    /// Stable machine-readable code (see [`codes`]).
    pub code: String,
    /// Severity band.
    pub severity: Severity,
    /// Scope the finding applies to (`store` or `bank:<id>`).
    pub scope: String,
    /// Human-readable, PII-free explanation (counts and labels only).
    pub message: String,
}

impl OpsFinding {
    /// Build a finding from the code catalog.
    #[must_use]
    pub fn new(
        code: &str,
        severity: Severity,
        scope: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code: code.to_owned(),
            severity,
            scope: scope.into(),
            message: message.into(),
        }
    }
}

/// Aggregate `ok` bit over a set of findings.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Findings {
    /// Findings in emission order.
    pub findings: Vec<OpsFinding>,
}

impl Findings {
    /// Empty aggregate.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a finding.
    pub fn push(&mut self, finding: OpsFinding) {
        self.findings.push(finding);
    }

    /// True when no `Error`-severity finding is present (warnings/info pass).
    #[must_use]
    pub fn ok(&self) -> bool {
        !self.findings.iter().any(|f| f.severity == Severity::Error)
    }

    /// Highest severity present, if any.
    #[must_use]
    pub fn max_severity(&self) -> Option<Severity> {
        self.findings.iter().map(|f| f.severity).max()
    }

    /// Consume the aggregate, returning the findings in emission order.
    #[must_use]
    pub fn into_vec(self) -> Vec<OpsFinding> {
        self.findings
    }

    /// Number of findings.
    #[must_use]
    pub fn len(&self) -> usize {
        self.findings.len()
    }

    /// True when empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.findings.is_empty()
    }
}

/// Stable finding-code catalog shared across ops surfaces.
pub mod codes {
    /// Backend ping failed.
    pub const BACKEND_UNREACHABLE: &str = "BACKEND_UNREACHABLE";
    /// `schema_settings.schema_version` row missing.
    pub const SCHEMA_VERSION_MISSING: &str = "SCHEMA_VERSION_MISSING";
    /// Recorded schema version differs from the expected build version.
    pub const SCHEMA_VERSION_MISMATCH: &str = "SCHEMA_VERSION_MISMATCH";
    /// Derived vector/lexical index state could not be read.
    pub const VECTOR_INDEX_MISSING: &str = "VECTOR_INDEX_MISSING";
    /// Embedding sidecar health probe failed.
    pub const EMBEDDING_UNREACHABLE: &str = "EMBEDDING_UNREACHABLE";
    /// A system table read failed (corruption signal).
    pub const SYSTEM_TABLES_UNREADABLE: &str = "SYSTEM_TABLES_UNREADABLE";
    /// `assoc_edges` row whose endpoint is not a live item.
    pub const ORPHAN_ASSOC_EDGE: &str = "ORPHAN_ASSOC_EDGE";
    /// `triples` row whose carrier item is missing.
    pub const DANGLING_TRIPLE_REF: &str = "DANGLING_TRIPLE_REF";
    /// `triples.snapshot_ref` points at a missing item.
    pub const DANGLING_SNAPSHOT_REF: &str = "DANGLING_SNAPSHOT_REF";
    /// `triples.gist_ref` points at a missing item.
    pub const DANGLING_GIST_REF: &str = "DANGLING_GIST_REF";
    /// `memtree_nodes` leaf whose referenced item is missing.
    pub const ORPHAN_MEMTREE_LEAF: &str = "ORPHAN_MEMTREE_LEAF";
    /// Dense vector rows fewer than indexable items.
    pub const DENSE_COVERAGE_GAP: &str = "DENSE_COVERAGE_GAP";
    /// Lexical rows fewer than indexable items.
    pub const LEXICAL_COVERAGE_GAP: &str = "LEXICAL_COVERAGE_GAP";
    /// Export/import manifest inventory not available for drift comparison.
    pub const MANIFEST_UNAVAILABLE: &str = "MANIFEST_UNAVAILABLE";
    /// A repair action id is not in the allowlist.
    pub const UNKNOWN_REPAIR_ACTION: &str = "UNKNOWN_REPAIR_ACTION";
    /// A mutating repair/reindex call was not confirmed.
    pub const CONFIRM_REQUIRED: &str = "CONFIRM_REQUIRED";
    /// A referenced doctor plan id is not registered.
    pub const PLAN_NOT_FOUND: &str = "PLAN_NOT_FOUND";
    /// Dense rebuild requested without an embedding sidecar.
    pub const EMBEDDER_UNAVAILABLE: &str = "EMBEDDER_UNAVAILABLE";
    /// A repair/reindex action failed part-way through.
    pub const ACTION_FAILED: &str = "ACTION_FAILED";
    /// Non-interactive reindex aborted without completing.
    pub const REINDEX_ABORTED: &str = "REINDEX_ABORTED";
}

/// Scope label for a bank option (`store` when unscoped).
#[must_use]
pub fn scope_label(bank: Option<&str>) -> String {
    bank.map_or_else(|| "store".to_owned(), |b| format!("bank:{b}"))
}

/// Strip userinfo credentials from any URL embedded in an error message.
///
/// Driver errors can embed connection URLs with passwords; every ops surface
/// funnels error text through this one redactor so no caller can forget it.
///
/// A userinfo that carries only a user name is left alone. `postgres://clio@host/db`
/// has no password in it, and blanking it to `postgres://***@host/db` would put a
/// redaction mark on an address that never held a secret — which is what a
/// passwordless managed database looks like everywhere it is printed. The colon
/// is what separates the password from the user name, so its absence is the test.
///
/// Text without a `scheme://user:password@` prefix passes through unchanged.
#[must_use]
pub fn redact_credentials(message: &str) -> String {
    let Some(sep) = message.find("://") else {
        return message.to_owned();
    };
    let userinfo = &message[sep + 3..];
    let Some(at) = userinfo.find('@') else {
        return message.to_owned();
    };
    if !userinfo[..at].contains(':') {
        return message.to_owned();
    }
    format!("{}***{}", &message[..sep + 3], &userinfo[at..])
}

#[cfg(test)]
#[path = "finding_tests.rs"]
mod finding_tests;
