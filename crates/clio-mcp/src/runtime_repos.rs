// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the all-repository view of one concrete
//! backend store: the trait a runtime-state construction site uses to coerce
//! one `Arc<SqliteStore>` or `Arc<PostgresStore>` into each repository view
//! [`crate::runtime::McpState`] holds.
//!
//! ## Owns
//! - The `Repos` trait: the full set of repository interfaces one concrete
//!   backend store must implement to be usable as MCP runtime state
//!
//! ## Does not own
//! - Backend dispatch or engine assembly (see `runtime_open`)
//! - The `McpState` field set (see `runtime`)
//! - Any store behavior (see `clio-store`)
//!
//! ## Boundary
//!
//! Changes to which repository views a runtime state can be built over should
//! normally require changes here. Changes to what a view does should not.
//!
//! Keep this module focused on the combined repository bound.
//!
//! This split exists because `runtime_open` sits at the workspace file-size
//! limit, and the bound is the cohesive half that `McpState`'s field set, not
//! the construction sequence, decides.

use clio_store::{
    BatchStore, BeliefStore, ComplianceStore, ContinuousStore, DestinationCustodyStore,
    DurableRequestStore, HistoryStore, HygieneStore, IndexStore, LocalEventStore, OpsStore,
    PeerActionStore, PersonaStore, Store, SyncStore,
};

/// All-repository view of one concrete backend store.
pub(crate) trait Repos:
    Store
    + IndexStore
    + PersonaStore
    + BeliefStore
    + HistoryStore
    + ContinuousStore
    + ComplianceStore
    + HygieneStore
    + OpsStore
    + BatchStore
    + SyncStore
    + LocalEventStore
    + DestinationCustodyStore
    + DurableRequestStore
    + PeerActionStore
{
}

impl<
    T: Store
        + IndexStore
        + PersonaStore
        + BeliefStore
        + HistoryStore
        + ContinuousStore
        + ComplianceStore
        + HygieneStore
        + OpsStore
        + BatchStore
        + SyncStore
        + LocalEventStore
        + DestinationCustodyStore
        + DurableRequestStore
        + PeerActionStore,
> Repos for T
{
}
