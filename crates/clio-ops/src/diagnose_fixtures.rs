// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the shared fixtures the `diagnose` operator
//! tests build on: the fixed timestamp, an in-memory store, one episodic item,
//! and a long backend error.
//!
//! ## Owns
//! - `TS`, `sqlite`, `item`, and `failed`
//!
//! ## Does not own
//! - The diagnose surface or its findings (`diagnose`)
//! - The per-surface test doubles, which stay beside the tests that use them
//!
//! ## Boundary
//!
//! Changes to a diagnose fixture should normally require changes here. Changes
//! to diagnose behavior should not.
//!
//! Keep this module free of `#[test]` functions.

use clio_store::SqliteStore;
use clio_types::{AmError, EpisodicType, EpistemicKind, ErrorCode, ItemKind, MemoryItem};
use serde_json::json;
use std::sync::Arc;

/// The fixed timestamp every diagnose fixture stamps.
pub(crate) const TS: &str = "2026-01-01T00:00:00Z";

/// An in-memory store over a process-local KMS.
pub(crate) fn sqlite() -> SqliteStore {
    SqliteStore::open_in_memory(Arc::new(clio_store::LocalDevKms::new())).expect("in-memory store")
}

/// One episodic task item with a snapshot the diagnose probes read.
pub(crate) fn item(id: &str, bank: &str) -> MemoryItem {
    MemoryItem {
        id: id.to_owned(),
        bank_id: bank.to_owned(),
        subject_id: format!("subj-{bank}"),
        kind: ItemKind::Episodic,
        category: None,
        episodic_type: Some(EpisodicType::Task),
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.9),
        admission_score: Some(0.6),
        source_ref: Some("turn_1".to_owned()),
        snapshot: Some(json!({"error_code": "ECONNRESET"})),
        gist: Some("diagnose fixture".to_owned()),
        valid_from: TS.to_owned(),
        valid_until: None,
        tx_from: TS.to_owned(),
        tx_until: None,
        created_at: TS.to_owned(),
        updated_at: TS.to_owned(),
        context: None,
    }
}

/// A long backend error, so the report's message truncation is exercised.
pub(crate) fn failed(what: &str) -> AmError {
    let mut text = String::from("\n");
    text.push_str(&what.repeat(60));
    AmError::new(ErrorCode::Internal, text)
}
