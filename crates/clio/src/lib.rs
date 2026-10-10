// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the crate root: public module graph and package identity.
//!
//! ## Owns
//! - Crate module declarations and public re-exports
//! - Package `name` / `version` / `identity` helpers
//!
//! ## Does not own
//! - Effective configuration merge or tool handlers
//! - Persistence, MCP transports, or memory CRUD
//!
//! ## Boundary
//!
//! Changes to the public crate surface and identity APIs should normally require changes here.
//! Changes to config, ranking, or profile internals should not require changes here.
//!
//! Keep this module focused on the crate root surface and package identity.

#![forbid(unsafe_code)]

pub use clio_admission as admission;
pub use clio_belief as belief;
pub use clio_config::config;
pub use clio_config::log;
pub use clio_config::profile;
pub use clio_config::ranking;
pub use clio_config::retention;
pub use clio_config::retention_schema;
pub use clio_config::retention_store;
pub use clio_config::secret;
pub use clio_history as history;
pub use clio_hygiene as hygiene;
pub use clio_index as index;
pub use clio_persona as persona;
pub use clio_retrieve as retrieve;
pub use clio_store as store;
pub use clio_types::context;
pub use clio_types::error;
pub use clio_write as write;

pub use clio_belief::{
    BeliefHistory, BeliefObserveArgs, BeliefObserveOutcome, BeliefSignalSource, belief_history,
    belief_observe,
};
pub use clio_config::{CONFIG_SCHEMA_VERSION, ConfigScope, Runtime};
pub use clio_config::{Profile, ToolImplStatus, ToolInventoryEntry, coding_tool_inventory};
pub use clio_history::{
    FailureQueryOutcome, FailureRecordOutcome, HistoryEvent, HistoryVecSink, PrepareRetryOutcome,
    RETITLE_FIXTURES, TaskGetOutcome, TaskHistoryOutcome, TaskUpsertOutcome,
    TemporalHistoryOutcome, failure_record, failures_for_task, prepare_retry, task_get,
    task_history, task_upsert, temporal_history,
};
pub use clio_index::{DenseSearchArgs, IndexCoordinator, LexicalSearchArgs};
pub use clio_persona::{
    PersonaGetArgs, PersonaGetOutcome, PersonaObserveOutcome, PersonaObservePreferenceArgs,
    PersonaPutStableArgs, PersonaPutStableOutcome, persona_get, persona_observe_preference,
    persona_put_stable,
};
pub use clio_types::{Actor, Bank, BankDefaultPolicy, OpContext};
pub use clio_types::{AmError, ErrorCode, ToolResult};
pub use clio_types::{
    BeliefObject, ConfidenceEntry, EpisodicType, EpistemicKind, ItemKind, MemoryItem, RetrieveHit,
    SemanticCategory, SourceType, TimeAxis, TripleQuery, TripleRecord, UpdateRule,
};
pub use clio_write::{RawIngestOptions, ingest_raw, ingest_raw_with_extractor};

/// Package version from `Cargo.toml`.
#[must_use]
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[must_use]
pub fn name() -> &'static str {
    "clio"
}

/// Human-readable identity line for CLI / health probes.
#[must_use]
pub fn identity() -> String {
    format!("{} {}", name(), version())
}

#[cfg(test)]
mod tests {
    #[test]
    fn version_should_be_non_empty() {
        let version = crate::version();
        assert!(!version.is_empty(), "{version:?}");
    }

    #[test]
    fn name_should_equal_clio() {
        assert_eq!(crate::name(), "clio");
    }

    #[test]
    fn identity_includes_name_and_version() {
        let id = crate::identity();
        assert!(id.contains("clio"));
        assert!(id.contains(crate::version()));
    }
}
