// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is the `clio-write` crate root: extraction, span verify, gated store,
//! parallel ingest, canonical consolidation, and MemTree maintenance.
//!
//! ## Owns
//! - Public module graph and re-exports for write-path orchestration
//!
//! ## Does not own
//! - Admission scoring math or dual-backend persistence drivers
//!
//! ## Boundary
//!
//! Changes to the write-path public surface should normally require changes here.
//! Changes to match rules or store drivers should not require changes here.
//!
//! Keep this module focused on the crate root surface for write orchestration.

#![deny(missing_docs)]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::missing_panics_doc)]
#![allow(clippy::must_use_candidate)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::doc_markdown)]

pub mod chunk;
pub mod consolidate;
pub mod discard;
pub mod extract;
pub mod extract_chat;
pub mod extract_factory;
pub mod hub_distill;
mod hub_distill_claim;
mod hub_distill_model;
#[cfg(test)]
mod hub_distill_test_support;
pub mod ingest;
pub mod maintenance;
pub mod memtree;
pub mod memtree_attach;
pub mod memtree_maint;
pub mod memtree_tools;
pub mod parallel;
pub mod persona;
pub mod pipeline;
pub mod raw_ingest;
pub mod schema;
pub mod store_path;
pub mod telemetry;
pub mod triple;
pub mod update;
pub mod verify;
pub mod verify_date;
pub mod verify_entity;
pub mod verify_number;

pub use chunk::{Chunk, Turn, partition_session};
pub use consolidate::{
    CanonicalUnit, ConsolidateResult, VerifiedCandidate, consolidate, discrete_key, fingerprint,
    normalize_surface,
};
pub use discard::{
    DiscardArgs, DiscardEvent, DiscardOutcome, DiscardSink, DiscardVecSink, discard,
};
pub use extract::{
    DEFAULT_EXTRACT_TIMEOUT, ExtractCandidate, ExtractRequest, Extractor, FixtureExtractor,
    HttpTransport, PoisonExtractor, TemplateApiExtractor, Transport,
};
pub use extract_chat::{
    ChatExtractor, MAX_CONTENT_BYTES, SYSTEM_PROMPT, build_user_message, chat_request_body,
    endpoint_url, parse_chat_response,
};
pub use extract_factory::build_extractor_from;
pub use hub_distill::{
    DEFAULT_HUB_NEIGHBORS, DistillOutcome, HUB_DISTILL_JOB, HUB_DISTILL_MARKER, HubDistillEngine,
    HubDistillReport,
};
pub use ingest::{
    AdmittedLeaf, IngestOutcome, IngestTiming, ParallelIngestArgs, WriteIngestConfig,
    parallel_ingest,
};
pub use maintenance::{
    BufferMaintenance, LeafIndex, MaintenanceHook, MaintenanceSignal, NoopMaintenance,
    SharedMaintenanceHook, dispatch_async,
};
pub use memtree::{
    AttachLeaf, BankMemTree, DEFAULT_FANOUT, MemForest, MemTreeNode, MemTreeQueryFilter,
};
pub use memtree_maint::{MaintStats, MemTreeMaint, SharedMemForest};
pub use memtree_tools::{
    ConsolidateAccept, MaintenanceStatusData, consolidate_tool, maintenance_status, memtree_get,
    memtree_query,
};
pub use parallel::{ChunkWorkerResult, ParallelExtractAggregate, extract_chunks_parallel};
pub use pipeline::{
    ExtractStoreArgs, ExtractStoreOutcome, ExtractVerifyArgs, ExtractVerifyOutcome,
    NonAuthoritativeRetention, extract_and_verify, extract_verify_store,
};
pub use raw_ingest::{EmptySignals, RawIngestOptions, ingest_raw, ingest_raw_with_extractor};
pub use schema::{ExtractiveKind, FieldSpec, default_event_fields, json_path};
pub use store_path::{
    VerifiedStoreRequest, admit_preview_verified, apply_evidence_ref, gated_store_verified,
};
pub use telemetry::{VerifyEvent, VerifySink, VerifyVecSink};
pub use triple::{
    TripleAddArgs, TripleEvent, TripleSink, TripleVecSink, default_time_axis, invalidate,
    triple_add, triple_end, triple_query,
};
pub use update::{
    ContinuousEvent, ContinuousSink, ContinuousVecSink, UpdateArgs, UpdateOutcome, update,
};
pub use verify::{SpanOffsets, VerifyConfig, VerifyFailure, VerifyReport, verify_snapshot};
