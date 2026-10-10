// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module holds the fixtures shared by the store-path test modules: the
//! context builders, the item builder, and the signal source that answers
//! nothing.
//!
//! ## Owns
//! - `ctx` / `bound_ctx`, an unbound and a credential-bound operation context
//! - `item_with_snapshot`, a verified-candidate item
//! - `EmptyNeighbors`, a `SignalSource` with no neighbours
//!
//! ## Does not own
//! - Any test
//! - The gated store path itself (`store_path.rs`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use clio_admission::gated::SignalSource;
use clio_types::{
    Actor, AmError, Bank, EpistemicKind, ItemKind, MemoryItem, OpContext, PrincipalLabel,
    SemanticCategory,
};

/// An operation with no presenting credential (the local-transport case).
pub(super) fn ctx() -> OpContext {
    OpContext::new(Bank::new("bank-1"), Actor::Agent)
}

/// The same operation presented by a named credential.
pub(super) fn bound_ctx(label: &str) -> OpContext {
    OpContext::with_principal(
        Bank::new("bank-1"),
        Actor::Agent,
        PrincipalLabel::new(label),
    )
}

/// A verified candidate: the shape the gated store path admits.
pub(super) fn item_with_snapshot(id: &str, snap: serde_json::Value) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: "bank-1".into(),
        subject_id: "subj".into(),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::Persona),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        admission_score: None,
        source_ref: None,
        snapshot: Some(snap),
        gist: Some("gist".into()),
        valid_from: "t0".into(),
        valid_until: None,
        tx_from: "t0".into(),
        tx_until: None,
        created_at: "t0".into(),
        updated_at: "t0".into(),
        context: None,
    }
}

/// A signal source that reports no neighbours and no prior age.
#[derive(Debug, Default)]
pub(super) struct EmptyNeighbors;

impl SignalSource for EmptyNeighbors {
    fn neighbor_texts(&self, _item: &MemoryItem) -> Result<Vec<String>, AmError> {
        Ok(vec![])
    }
}
