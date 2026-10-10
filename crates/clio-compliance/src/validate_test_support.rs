// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module holds the fixtures shared by the reviewer-validation test
//! modules: the store, the item, the trail reader, and the two argument
//! builders.
//!
//! ## Owns
//! - `store`, a fresh in-memory SQLite store per call
//! - `item`, one memory item for a bank
//! - `trail`, the operation labels an item's audit trail holds
//! - `args` / `bound_args`, a reviewer action with or without a credential
//!
//! ## Does not own
//! - Any test
//! - The reviewer action routing itself (`validate.rs`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::validate::{ValidateAction, ValidateArgs};
use clio_store::{DekProvider, LocalDevKms, SqliteStore, Store};
use clio_types::{EpistemicKind, ItemKind, MemoryItem, PrincipalLabel, SemanticCategory};
use std::sync::Arc;

/// A fresh in-memory store, so no test depends on another.
pub(super) fn store() -> SqliteStore {
    let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
    SqliteStore::open_in_memory(kms).unwrap()
}

/// One memory item for a bank.
pub(super) fn item(id: &str, bank: &str, subject: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: subject.into(),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::parse("schema").unwrap()),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.8),
        admission_score: Some(0.8),
        source_ref: None,
        snapshot: Some(serde_json::json!({"endpoint": "https://api"})),
        gist: Some("the API endpoint is https://api".into()),
        valid_from: "2026-01-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2026-01-01T00:00:00Z".into(),
        tx_until: None,
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-01-01T00:00:00Z".into(),
        context: None,
    }
}

/// The operation labels an item's audit trail holds, in trail order.
pub(super) fn trail(store: &SqliteStore, bank: &str, item_id: &str) -> Vec<String> {
    store
        .audit_trail(bank, item_id)
        .unwrap()
        .events
        .iter()
        .map(|e| e.operation.clone())
        .collect()
}

/// One reviewer action presented by no credential (the local-transport case).
#[allow(clippy::too_many_arguments)]
pub(super) fn args<'a>(
    store: &'a SqliteStore,
    bank: &'a str,
    item_id: &'a str,
    action: ValidateAction,
    note: Option<&'a str>,
    new_content: Option<&'a str>,
    new_item_id: &'a str,
) -> ValidateArgs<'a> {
    bound_args(
        store,
        bank,
        item_id,
        action,
        note,
        new_content,
        new_item_id,
        None,
    )
}

/// The same reviewer action, optionally presented by a named credential.
#[allow(clippy::too_many_arguments)]
pub(super) fn bound_args<'a>(
    store: &'a SqliteStore,
    bank: &'a str,
    item_id: &'a str,
    action: ValidateAction,
    note: Option<&'a str>,
    new_content: Option<&'a str>,
    new_item_id: &'a str,
    principal: Option<&'a PrincipalLabel>,
) -> ValidateArgs<'a> {
    ValidateArgs {
        store,
        continuous: store,
        bank_id: bank,
        actor: "harness",
        principal,
        item_id,
        action,
        note,
        new_content,
        timestamp: "2026-01-02T00:00:00Z",
        new_item_id,
    }
}
