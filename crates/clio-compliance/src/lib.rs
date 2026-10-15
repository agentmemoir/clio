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
//! - `scope_inventory` read-only local compliance scope ledger
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

mod action_event_log;
mod audit;
mod bundle;
mod canonical;
mod copy_disposition;
mod correct;
mod erase;
mod erasure_journal;
mod erasure_journal_file;
mod export;
mod export_custody;
mod export_mask;
mod export_subject;
mod import;
mod inventory;
#[cfg(test)]
mod local_action_probe;
mod local_review;
mod local_review_export;
mod scratchpad;
mod stats;
pub mod subject_bank_coordination;
mod subject_request;
mod subject_request_events;
mod subject_request_frozen;
mod subject_request_manifest;
pub mod subject_request_store_wiring;
mod validate;

pub use action_event_log::{ActionEventLog, RecordOutcome};
pub use audit::{AuditTrailView, audit_trail};
pub use bundle::{
    BUNDLE_FORMAT, BUNDLE_FORMAT_VERSION, BundleEnvelope, CompletenessManifest, ExportContentMode,
    ExportEntities, OpaqueItem, ParsedBundle, canonical_entities_bytes, canonical_value_bytes,
    entities_checksum, parse_bundle, sha256_hex_bytes, validate_envelope,
};
pub use canonical::{CanonicalValue, canonical_get, canonical_subject};
pub use copy_disposition::{
    CopyAction, CopyClosureFacts, CopyCustodyClass, CopyDisposition, CopyDispositionOutcome,
    CopyOwner, CopyRequiredAction, CopyState, copy_closure_facts,
};
pub use correct::{
    BeliefContextCorrectArgs, BeliefContextCorrector, CorrectArgs, CorrectOutcome, CorrectResult,
    correct, correct_belief_context,
};
pub use erase::{
    DEFAULT_REASON_CODE, EraseArgs, EraseEvent, EraseOutcome, EraseSink, EraseVecSink,
    erase_request, tombstone_salt, tombstone_salt_from,
};
pub use erasure_journal::{
    ArchiveClaim, AuthorityRefusal, DenialRecord, ErasureJournal, JournalState, MemberFacts,
    SCHEMA_VERSION, check_archive_authority, handoff, initialize_new_empty, load_journal,
    record_denial,
};
pub use export::{ExportArgs, ExportDelivery, ExportFilter, ExportOutcome, export};
pub use export_custody::{
    CustodyState, DeliveryRefusal, DeliveryResult, DeliverySpec, OpenDestination, deliver_export,
    reconcile_open_destinations,
};
pub use import::{ImportArgs, ImportReport, import};
pub use inventory::{
    Custody, InventoryBackend, InventoryFacts, InventoryInput, InventoryPaths, InventoryRow, Owner,
    Readability, RetentionAction, ScopeInventory, scope_inventory,
};
pub use local_review::{
    DEFAULT_REVIEW_LIMIT, LocalReviewPage, LocalReviewQuery, MAX_REVIEW_LIMIT, ReviewOutputRoute,
    record_review_output_custody, review_local_events, review_output_routes,
};
pub use local_review_export::write_review_export;
pub use scratchpad::{MAX_KEYS, MAX_TOTAL_BYTES, Scratchpad, TTL};
pub use stats::{PressureCounts, StatsInputs, StatsReport, stats};
pub use subject_request::{
    FinalOutcome, PathOwner, PathSlot, PathState, PeerState, RegisterOutcome, RequestAction,
    RequestActionKind, RequestActionState, RequestCommand, RequestKind, RequestScope, RequestState,
    StateCategory, SubjectRequestLedger, SubjectRequestRecord, VerificationResult, transition,
};
pub use subject_request_events::{
    ACTION_EVENT_TYPE, ACTION_EVENT_VERSION, ActionEventKind, ActionIntent, ActionOutcome,
    ActionPayload, BindingControls, BindingRefusal, ClosureBlock, ClosureInputs, PendingResult,
    ProtectedAction, RestoreInputs, RestoreRefusal, SubjectActionEvent, accept_closure,
    check_binding, check_restore,
};
pub use subject_request_manifest::{
    CopyCustodyManifest, DIGEST_ALGORITHM_PREFIX, KEY_COPY_DIGEST, KEY_PRODUCED_AT, KEY_PRODUCER,
    KEY_RECEIVED_AT, KEY_RECEIVER,
};
pub use validate::{ValidateAction, ValidateArgs, ValidateOutcome, validate};

#[cfg(test)]
// Re-exported so the sibling suites keep their crate-root import paths;
// the suites themselves are declared by their owning production files.
pub(crate) use export_subject::export_subject_scenarios_tests;
#[cfg(test)]
mod export_dest_fixtures;
#[cfg(test)]
mod export_tests;
#[cfg(test)]
mod import_fixture;
#[cfg(test)]
mod import_fixture_signals;
#[cfg(test)]
// Renamed from `inventory_fixture.rs` so the gate-boundary naming exemption
// (`*_fixtures.rs`) applies; the module keeps its old name because the test
// modules import through `crate::inventory_fixture`.
#[path = "inventory_fixtures.rs"]
mod inventory_fixture;
#[cfg(test)]
mod inventory_map_fixtures;
#[cfg(test)]
mod inventory_pg_support_tests;
#[cfg(test)]
#[path = "inventory_pg_tests.rs"]
mod inventory_pg_tests;
#[cfg(test)]
pub(crate) use local_review_export::review_export_scenarios_tests;
#[cfg(test)]
mod subject_request_events_tests;
#[cfg(test)]
mod test_unique;
