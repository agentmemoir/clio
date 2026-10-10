// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is the `clio-hygiene` crate root: operational anti-bloat tools
//! — deterministic noise scoring, the read-only
//! `hygiene_audit` ranked scan, the confirmed `hygiene_clean` mutator, and the
//! durable hygiene audit log.
//!
//! ## Owns
//! - Noise pattern packs and secret-class detection over decrypted item text
//! - `hygiene_noise_score` composition, reasons, and suggested-action bands
//! - `hygiene_audit`: bank-scoped ranked candidates with masked previews
//!   (zero writes, deterministic under fixed config)
//! - `hygiene_clean`: dry-run counting vs confirmed `flag`/`archive`/
//!   `discard` (`delete` alias) with per-item hygiene-log rows
//!
//! ## Does not own
//! - Backend SQL for flags, archive markers, or the log (`clio-store`)
//! - Compliance erasure; hygiene never destroys DEKs or writes tombstones
//! - MCP schemas, masking plumbing, or confirmation transport (`clio-mcp`)
//!
//! ## Boundary
//!
//! Changes to scoring formulas, detector heuristics, audit ranking, or clean
//! orchestration should normally require changes here. Changes to storage SQL
//! or tool bindings should not require changes here.
//!
//! Keep this crate focused on the hygiene audit/clean services.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::missing_panics_doc)]
#![allow(clippy::module_name_repetitions)]

mod audit;
mod clean;
mod detect;
mod scoring;

pub use audit::{
    AuditArgs, AuditReport, DEFAULT_MAX_SCAN, DEFAULT_MIN_SCORE, NoiseCandidate, hygiene_audit,
};
pub use clean::{
    CleanAction, CleanArgs, CleanOutcome, CleanTarget, clean, targets_from_candidates,
};
pub use detect::{MAX_PREVIEW_CHARS, NoiseSignals, detect_noise};
pub use scoring::{ScoreInput, Scored, score_noise};
