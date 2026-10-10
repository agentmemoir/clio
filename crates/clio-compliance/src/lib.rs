// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is the `clio-compliance` crate root: the transparency, inspection,
//! correction, verified-erasure surface over existing write-path data
//! Plus the additive harness workspace tools
//! (harness workspace tools): ephemeral scratchpad, single-slot canonical slots,
//! reviewer validate, and read-only stats.
//!
//! ## Owns
//! - `audit_trail` reconstruction entrypoint with secret masking
//! - `correct` routing: discrete item corrections vs continuous attribute
//!   adjustments, with mandatory reason validation
//! - `erase_request` verified crypto-shredding workflow and its authorization gate
//! - `Scratchpad` ephemeral session workspace with quotas and TTL
//! - `canonical_put` / `canonical_get` single-slot adapter
//! - `validate` reviewer actions (attest / update / invalidate / discard)
//! - `stats` read-only bank/tier/category reporter
//!
//! ## Does not own
//! - Backend SQL (`clio-store` owns both backends)
//! - MCP argument decoding and tool schema publication (`clio-mcp`)
//! - Operational noise cleanup (hygiene) and discard paths (other phases own those)
//!
//! ## Boundary
//!
//! Changes to the correction routing, audit masking, erasure workflow,
//! scratchpad quotas, canonical slots, reviewer routing, or stats shape should
//! normally require changes here. Changes to storage SQL or MCP bindings should
//! not require changes here.
//!
//! Keep this crate focused on the audit / correct / erase / workspace services.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::missing_panics_doc)]
#![allow(clippy::module_name_repetitions)]

mod audit;
mod bundle;
mod canonical;
mod correct;
mod erase;
mod export;
mod export_mask;
mod import;
mod scratchpad;
mod stats;
mod validate;

pub use audit::{AuditTrailView, audit_trail};
pub use bundle::{
    BUNDLE_FORMAT, BUNDLE_FORMAT_VERSION, BundleEnvelope, CompletenessManifest, ExportContentMode,
    ExportEntities, OpaqueItem, ParsedBundle, canonical_entities_bytes, canonical_value_bytes,
    entities_checksum, parse_bundle, sha256_hex_bytes, validate_envelope,
};
pub use canonical::{CanonicalValue, canonical_get, canonical_subject};
pub use correct::{
    BeliefContextCorrectArgs, BeliefContextCorrector, CorrectArgs, CorrectOutcome, CorrectResult,
    correct, correct_belief_context,
};
pub use erase::{
    DEFAULT_REASON_CODE, EraseArgs, EraseEvent, EraseOutcome, EraseSink, EraseVecSink,
    erase_request, tombstone_salt, tombstone_salt_from,
};
pub use export::{ExportArgs, ExportFilter, ExportOutcome, export};
pub use import::{ImportArgs, ImportReport, import};
pub use scratchpad::{MAX_KEYS, MAX_TOTAL_BYTES, Scratchpad, TTL};
pub use stats::{PressureCounts, StatsInputs, StatsReport, stats};
pub use validate::{ValidateAction, ValidateArgs, ValidateOutcome, validate};

#[cfg(test)]
mod export_tests;
#[cfg(test)]
mod import_fixture;
#[cfg(test)]
mod import_fixture_signals;
#[cfg(test)]
mod test_unique;
