// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is the `clio-ops` crate root: PII-safe operator health surfaces
//! `diagnose`, `verify`, `doctor`, `repair`, and
//! `reindex`.
//!
//! ## Owns
//! - The stable `ops_finding` shape and finding-code catalog
//! - `diagnose`: read-only install/DB health snapshot
//! - `verify`: referential-integrity and index-coverage pass/fail findings
//! - `doctor`: ordered repair-plan composition plus the plan registry
//! - `repair`: confirmation-gated allowlisted execution with re-verification
//! - `reindex`: public dry-run/live UX over the derived-index rebuild API
//!
//! ## Does not own
//! - Backend operator SQL (`clio_store::OpsStore`)
//! - The rebuild sweep (`clio_index::rebuild`)
//! - MCP schemas/transports or CLI exit plumbing (`clio-mcp` / `clio`)
//!
//! ## Boundary
//!
//! Changes to finding codes, check logic, plan composition, or repair policy
//! should normally require changes here. Changes to storage SQL or tool
//! bindings should not require changes here.
//!
//! Every surface is PII-safe: findings carry codes, severities, scopes, counts,
//! and labels only — never memory content, item snapshots, DEKs, or API keys.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::missing_panics_doc)]
#![allow(clippy::must_use_candidate)]
#![allow(clippy::module_name_repetitions)]

pub mod diagnose;

#[cfg(test)]
mod diagnose_fixtures;
pub mod doctor;
pub mod finding;
pub mod reindex;
pub mod repair;
pub mod verify;

pub use diagnose::{DiagnoseArgs, DiagnoseReport, VERIFY_POINTER, diagnose, diagnose_scoped};
pub use doctor::{
    ACTION_DROP_ORPHAN_ASSOC_EDGE, ACTION_REBUILD_DENSE_INDEX, ACTION_REBUILD_LEXICAL_INDEX,
    DoctorArgs, DoctorPlan, PlanRegistry, RepairAction, doctor, is_known_action, known_actions,
};
pub use finding::{Findings, OpsFinding, Severity, codes, redact_credentials, scope_label};
pub use reindex::{ReindexArgs, ReindexReport, reindex, target_label};
pub use repair::{ActionOutcome, ActionStatus, RepairArgs, RepairOutcome, repair};
pub use verify::{VerifyArgs, VerifyReport, error_codes, verify};
