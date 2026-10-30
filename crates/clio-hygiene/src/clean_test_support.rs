// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module holds the fixtures shared by the clean test modules: the store,
//! the item, the target, and the two operation contexts.
//!
//! ## Owns
//! - `store`, a fresh in-memory SQLite store per call
//! - `item`, `target`, and `args`, the shapes one clean call needs
//! - `ctx` / `bound_ctx`, an unbound and a credential-bound operation context
//!
//! ## Does not own
//! - Any test
//! - The clean orchestration itself (`clean.rs`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::clean::{CleanAction, CleanArgs, CleanTarget};
use clio_store::{LocalDevKms, SqliteStore};
use clio_types::{
    Actor, Bank, EpistemicKind, ItemKind, MemoryItem, OpContext, PrincipalLabel, SemanticCategory,
};
use std::sync::Arc;

/// A fresh in-memory store, so no test depends on another.
pub(crate) fn store() -> Arc<SqliteStore> {
    SqliteStore::open_in_memory(Arc::new(LocalDevKms::new()))
        .map(Arc::new)
        .expect("open sqlite")
}

/// One memory item for a bank.
pub(crate) fn item(id: &str, bank: &str, gist: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: format!("subj-{id}"),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::ToolConfig),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.8),
        admission_score: Some(0.5),
        source_ref: Some("turn_9".into()),
        snapshot: None,
        gist: Some(gist.into()),
        valid_from: "2026-06-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2026-06-01T00:00:00Z".into(),
        tx_until: None,
        created_at: "2026-06-01T00:00:00Z".into(),
        updated_at: "2026-06-01T00:00:00Z".into(),
        context: None,
    }
}

/// An operation with no presenting credential (the local-transport case).
pub(crate) fn ctx(bank: &str) -> OpContext {
    OpContext::new(Bank::new(bank), Actor::Agent)
}

/// The same operation presented by a named credential.
pub(crate) fn bound_ctx(bank: &str, label: &str) -> OpContext {
    OpContext::with_principal(Bank::new(bank), Actor::Agent, PrincipalLabel::new(label))
}

/// One clean target.
pub(crate) fn target(id: &str) -> CleanTarget {
    CleanTarget {
        item_id: id.into(),
        noise_score: Some(0.75),
        suggested_action: Some("archive".into()),
    }
}

/// One clean call's arguments.
pub(crate) fn args<'a>(
    store: &'a SqliteStore,
    opctx: &'a OpContext,
    targets: &'a [CleanTarget],
    action: CleanAction,
    confirm: bool,
) -> CleanArgs<'a> {
    CleanArgs {
        store,
        hygiene: store,
        ctx: opctx,
        targets,
        action,
        confirm,
        reason: "hygiene noise cleanup",
        timestamp: "2026-09-20T00:00:00Z",
        batch_id: "batch-1",
        note: Some("review me"),
    }
}
