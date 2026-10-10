// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for index pipeline report/status value shapes.
//!
//! ## Owns
//! - `RebuildTargets`, `RebuildReport`, `ProcessReport`, `IndexStatus`,
//!   `EmbedStatus` report types
//!
//! ## Does not own
//! - Coordinator behavior (`worker`)
//! - Coverage counting SQL (`clio_store::IndexStore`)
//!
//! ## Boundary
//!
//! Changes to report shapes should normally require changes here.
//! Changes to worker/rebuild policy should not require changes here.

use clio_store::IndexCoverage;
use clio_types::{AmError, ErrorCode};

/// Which derived indexes a rebuild refreshes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RebuildTargets {
    /// Dense vectors only.
    Dense,
    /// Lexical documents only.
    Lexical,
    /// Both legs.
    All,
}

impl RebuildTargets {
    /// Parse wire value.
    ///
    /// # Errors
    /// Unknown target strings.
    pub fn parse(raw: &str) -> Result<Self, AmError> {
        match raw {
            "dense" => Ok(Self::Dense),
            "lexical" => Ok(Self::Lexical),
            "all" => Ok(Self::All),
            other => Err(AmError::new(
                ErrorCode::InvalidArgument,
                format!("rebuild targets must be dense|lexical|all, got `{other}`"),
            )),
        }
    }
}

/// Sidecar + pipeline snapshot (PII-safe: ids/labels only, never memory text).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbedStatus {
    /// Embedding model id recorded on vector rows (`""` in lexical-only mode).
    pub model_id: String,
    /// Configured vector dimension (`0` in lexical-only mode).
    pub dims: usize,
    /// Index mode serving the active width (`ann_vector`/`ann_halfvec`/`exact_scan`).
    pub index_mode: String,
    /// `false` when no embedding provider is configured: lexical only.
    pub dense_available: bool,
    /// Wall-clock milliseconds of the last embed call in this process.
    pub last_latency_ms: Option<u128>,
    /// Last worker error text (ids/labels only).
    pub last_error: Option<String>,
}

/// Index pipeline health report (counts only; PII-safe by construction).
#[derive(Debug, Clone, PartialEq)]
pub struct IndexStatus {
    /// Coverage counts for the requested scope.
    pub coverage: IndexCoverage,
    /// Sidecar snapshot.
    pub embed: EmbedStatus,
}

/// One `process_pending` sweep result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ProcessReport {
    /// Jobs whose dense + lexical rows were applied.
    pub applied: u32,
    /// Jobs whose lexical doc was written with no embedder configured.
    pub lexical_only: u32,
    /// Jobs skipped without error (item gone, legacy format).
    pub skipped: u32,
    /// Jobs that failed and stay pending with backoff.
    pub failed: u32,
}

/// One `rebuild_indexes` result (dry-run or apply).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RebuildReport {
    /// Scope bank (`None` = whole store).
    pub bank: Option<String>,
    /// Whether the run wrote anything.
    pub dry_run: bool,
    /// Open, non-discarded indexable items in scope.
    pub indexable_items: u64,
    /// Indexable items missing a dense row before the sweep.
    pub dense_missing: u64,
    /// Indexable items missing a lexical row before the sweep.
    pub lexical_missing: u64,
    /// Dense rows written during the sweep (0 on dry-run).
    pub dense_applied: u64,
    /// Lexical rows written during the sweep (0 on dry-run).
    pub lexical_applied: u64,
    /// Derived rows for vanished items removed (0 on dry-run).
    pub orphans_removed: u64,
}
