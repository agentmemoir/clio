// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is the `clio-store` crate root for dual-backend persistence:
//! Postgres+pgvector and SQLite+sqlite-vec. `subject_id` means the
//! GDPR/data-subject key for DEKs — not an SPO triple subject.
//!
//! ## Owns
//! - The module graph and the public re-exports of the store API
//! - The crate-wide lint attributes
//!
//! ## Does not own
//! - Any store behaviour, which lives in the module that implements it
//! - The shared types, which live in `clio-types`
//!
//! ## Boundary
//!
//! Changes to which modules or names are public should normally require
//! changes here. Changes inside a module should not require changes here.
//!
//! Keep this module focused on wiring and re-exporting.

#![deny(missing_docs)]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::missing_panics_doc)]
#![allow(clippy::must_use_candidate)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::cast_possible_wrap)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::type_complexity)]
#![allow(clippy::redundant_closure_for_method_calls)]
#![allow(clippy::match_same_arms)]
#![allow(clippy::doc_markdown)]
#![allow(clippy::unnecessary_lazy_evaluations)]
#![allow(clippy::redundant_clone)]
#![allow(clippy::option_if_let_else)]
#![allow(clippy::manual_map)]
#![allow(clippy::map_unwrap_or)]

mod assoc;
mod async_store;
mod attribute_map;
mod audit_model;
mod bank_registration;
mod batch_model;
mod batch_postgres;
mod batch_sqlite;
#[cfg(test)]
mod batch_test_support_tests;
#[cfg(test)]
#[path = "batch_tests_persona.rs"]
mod batch_tests_persona;
mod belief_store;
mod belief_tx;
mod bootstrap;
mod compliance_store;
mod content_envelope;
mod content_read_errors;
mod continuous;
mod copy_inventory;
mod copy_inventory_converge;
mod db_url;
mod dek;
mod dek_slots;
mod derived_tables;
mod destination_custody;
mod distill_intent;
mod distilled_evidence;
mod distilled_freshness;
mod distilled_revision;
mod distilled_unit;
mod distilled_unit_codec;
mod distilled_unit_store;
mod durable_requests;
mod durable_requests_converge;
mod embedding_space;
mod erase_hash;
mod erasure_denial;
mod history_store;
mod hygiene_seq;
mod hygiene_store;
mod index_store;
mod inspect_query;
mod item_persist;
mod item_update;
mod keyed_digest;
mod kms_migrate;
mod lexical_digest;
mod lexical_porter;
mod local_events;
mod local_events_converge;
mod local_kms_db;
mod local_kms_migrate;
mod local_kms_schema;
mod memtree_model;
mod memtree_schema;
mod migrate;
mod model;
mod opaque_item_persist;
mod open_handles;
mod ops_store;
mod peer_actions;
mod persona_store;
mod postgres;
mod postgres_assoc;
mod postgres_attribute;
mod postgres_audit;
mod postgres_backup_model;
mod postgres_belief;
mod postgres_bootstrap;
mod postgres_correct;
mod postgres_coverage;
mod postgres_discard;
mod postgres_discovery;
mod postgres_distill_intent;
mod postgres_distilled_cascade;
mod postgres_distilled_claim;
mod postgres_distilled_unit;
mod postgres_distilled_unit_store;
mod postgres_dump;
mod postgres_envelope;
mod postgres_erase;
mod postgres_erase_read;
mod postgres_ext;
mod postgres_history_read;
mod postgres_history_write;
mod postgres_hygiene;
mod postgres_index;
mod postgres_inspect;
mod postgres_memory;
mod postgres_memtree;
mod postgres_ops;
mod postgres_persona;
mod postgres_restore;
mod postgres_restore_state;
mod postgres_store;
mod postgres_sync;
mod postgres_sync_records;
mod postgres_sync_rows;
mod postgres_triple;
mod postgres_triple_asserted;
mod postgres_triple_read;
mod postgres_window;
mod restore_key_swap;
mod restore_space;
mod restore_state;
mod restore_swap;
mod schema_converge;
mod schema_reshape;
mod snapshot_keys;
mod space_reconcile;
mod sqlite;
mod sqlite_assoc;
mod sqlite_attribute;
#[path = "sqlite_attribute_context.rs"]
mod sqlite_attribute_context;
mod sqlite_audit;
mod sqlite_belief;
mod sqlite_content;
mod sqlite_correct;
mod sqlite_coverage;
mod sqlite_discard;
mod sqlite_distill_intent;
mod sqlite_distilled_claim;
mod sqlite_distilled_unit;
mod sqlite_distilled_unit_store;
mod sqlite_envelope;
mod sqlite_erase;
mod sqlite_erase_purge;
mod sqlite_erase_triples;
mod sqlite_history_read;
mod sqlite_history_triple;
mod sqlite_history_write;
mod sqlite_hygiene;
mod sqlite_index;
mod sqlite_inspect;
mod sqlite_integrity;
mod sqlite_knn;
mod sqlite_memory;
mod sqlite_memtree;
mod sqlite_ops;
mod sqlite_path;
mod sqlite_persona;
mod sqlite_snapshot;
mod sqlite_store;
mod sqlite_sync;
mod sqlite_sync_rows;
mod sqlite_triple;
mod sqlite_triple_asserted;
mod sqlite_triple_read;
mod sqlite_window;
mod store;
mod store_lock;
mod store_lock_pids;
mod sync_records;
mod sync_store;
#[path = "sync_store_rows.rs"]
mod sync_store_rows;
mod triple_assertion;
mod triple_cipher;
mod triple_invariant;
mod triple_map;
mod triple_reshape;
mod vec_register;
mod vector_ddl;
mod window_query;

pub use assoc::{
    AssocPolicy, canonical_pair, coactivation_id, effective_weight, explicit_id, grow,
    parse_millis, should_prune,
};
pub use async_store::{AsyncStore, BlockingAsyncStore};
pub use audit_model::{
    AuditTrailEntry, BeliefConfidencePoint, CorrectionOutcome, CorrectionSpec, ItemAuditTrail,
    ItemRevision,
};
pub use batch_model::{BatchStore, BatchWrite, MAX_BATCH_OPS};
pub use belief_store::BeliefStore;
pub use bootstrap::{StoreBackend, StoreOpenOptions, open_store};
pub use compliance_store::{
    BankScopeStore, ComplianceStore, ComplianceTombstoneView, SubjectEraseOutcome, SubjectEraseSpec,
};
pub use content_read_errors::is_unreadable_content;
pub use continuous::ContinuousStore;
pub use copy_inventory::{CopyInventoryRow, CopyInventoryStore};
pub use dek::{CiphertextEnvelope, DekProvider, LocalDevKms};
pub use derived_tables::{TableDataClass, table_data_class};
pub use destination_custody::{DestinationCustodyRow, DestinationCustodyStore};
pub use distill_intent::DistillIntent;
pub use distilled_evidence::{EvidenceEntry, RevisionEvidence, evidence_from_items};
pub use distilled_freshness::{
    ClaimedSourceState, UnitFreshness, compute_freshness, unit_freshness,
};
pub use distilled_revision::{projected_unit_id, revision_item_id, snapshot_projection};
pub use distilled_unit::{
    DISTILLED_UNIT_RECORD_VERSION, DistilledUnitRecord, HUB_DISTILL_EDGE_RELATIONSHIP,
    STALE_REVISION_REJECTED, STALE_SOURCE_CHANGED, STALE_SOURCE_REMOVED,
};
pub use distilled_unit_store::DistilledUnitStore;
pub use durable_requests::{DurableRequestRow, DurableRequestStore};
pub use embedding_space::{
    ANN_HALFVEC_MAX_DIMS, ANN_VECTOR_MAX_DIMS, DEFAULT_EMBEDDING_MODEL, EmbeddingSpace, IndexMode,
    MAX_VECTOR_DIMS, SETTING_EMBEDDING_DIM, SETTING_EMBEDDING_MODEL,
};
pub use erase_hash::sha256_hex;
pub use erasure_denial::{DenialVerdict, ErasureDenial};
pub use history_store::HistoryStore;
pub use hygiene_store::{HygieneLogEntry, HygieneStore};
pub use index_store::IndexStore;
pub use keyed_digest::{keyed_digest, keyed_digest_hex};
pub use local_events::{LocalEventIntent, LocalEventState, LocalEventStore, LocalEventView};
pub use memtree_model::{MemTreeNodeRecord, MemTreeQuery};
pub use migrate::{
    SCHEMA_VERSION, canonical_optional_stamp, canonical_stamp, canonical_ts, canonical_write_stamp,
    is_epoch_stamp, millis_to_rfc3339, sqlite_vec_missing,
};
pub use model::{
    AssocEdgeRecord, AssocKind, EMBEDDING_DIMS, EmbeddingDims, IndexCoverage, IndexJob,
    InspectSource, InspectValidity, ItemIdentity, ItemInspectFilter, ItemInspectRow, KnnHit,
    OpaqueItemRecord, SearchHit, VectorSlot,
};
pub use ops_store::{IntegritySignals, OpsStore, StoreCounts};
pub use peer_actions::{
    PeerActionBinding, PeerActionDisposition, PeerActionKind, PeerActionRow, PeerActionState,
    PeerActionStore, PeerActionTarget,
};
pub use persona_store::PersonaStore;
pub use pg_probe::DatabaseReport;
pub use pg_tls::{
    Connection, SslChoice, connect as connect_postgres, is_loopback, parse_connection,
};
pub use postgres::PostgresStore;
pub use postgres_backup_model::{PgColumn, PgTable, compare_table_sets, member_name, quote_ident};
pub use postgres_dump::{
    PG_CONSISTENCY, PG_METHOD, PostgresDumpFacts, TableDump, snapshot_postgres_into,
};
pub use postgres_restore::{
    PostgresRestoreReport, PostgresRestoreRequest, RestoreTable, restore_postgres,
};
pub use postgres_restore_state::{PostgresTargetState, postgres_target_state};
pub use restore_key_swap::{KeySwapReport, place_staged_key};
pub use restore_space::{DiskSpace, SpaceCheck, SpaceFootprint, preflight_restore};
pub use restore_state::{SqliteFileState, sqlite_file_state, sqlite_file_state_unheld};
pub use restore_swap::{KeyPlacement, SwapReport, SwapRequest, swap_sqlite_store};
pub use snapshot_keys::{KeyMemberSource, key_member_source};
pub use space_reconcile::{SpaceReconcile, configured_embedding_space, space_switch_confirmed};
pub use sqlite::SqliteStore;
pub use sqlite_content::sqlite_content_digest;
pub use sqlite_integrity::sqlite_integrity_ok;
pub use sqlite_ops::sqlite_store_counts;
pub use sqlite_snapshot::{
    CONSISTENCY, METHOD, PAYLOAD_MEMBER, SnapshotFacts, snapshot_sqlite_into,
};
pub use store::Store;
pub use store_lock::ExclusiveHold;
pub use sync_store::{
    DeadLetterStatus, JournalAppendOutcome, SyncConflictRow, SyncDeadLetterRow, SyncFeedRow,
    SyncJournalRow, SyncStore,
};
pub use triple_assertion::TripleAssertion;
pub use vector_ddl::{
    HALFVEC_MIN_VERSION, pgvector_supports_halfvec, postgres_vector_ddl, sqlite_vector_ddl,
};

pub use clio_types::{
    AttributeSchema, ContinuousPoint, ContinuousState, SearchDomain, TimeAxis, TripleAddOutcome,
    TripleEndOutcome, TripleQuery, TripleRecord, UpdateRule, ensure_discrete_update_rule,
    interval_contains,
};

#[cfg(test)]
mod erase_test_support;

mod pg_advisory_keys;
mod pg_lock;
mod pg_probe;
mod pg_scoped_schema;
#[cfg(any(test, feature = "test-support"))]
pub mod pg_skip;
#[cfg(any(test, feature = "test-support"))]
pub mod pg_test_target;
mod pg_tls;
mod pg_url;
mod tls_trust;

#[cfg(test)]
mod test_unique;

/// Serialize tests that share `CLIO_DATABASE_URL` / Compose Postgres across
/// crates: every shared-Postgres test takes this guard first. A helper that
/// opens the shared database must *return* this guard to its caller rather than
/// take it locally, or the lock is released the moment the helper returns and
/// the caller's queries run unguarded.
pub use pg_lock::{PgTestGuard, lock_pg_test};
pub use pg_probe::probe as probe_postgres;
/// Give one test a private Postgres schema: the harness for assertions over a
/// store-global view (a drain report, coverage counts) on the shared database.
pub use pg_scoped_schema::ScopedSchema;
