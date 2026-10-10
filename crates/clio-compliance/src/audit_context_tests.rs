// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for source-`context` coverage of the
//! `audit_trail` view: secret-shaped context is scrubbed before the trail
//! leaves the service, non-secret context stays readable, and telemetry keeps
//! only presence/length/hash.
//!
//! ## Owns
//! - Context-focused audit-view assertions over an in-memory SQLite store
//!
//! ## Does not own
//! - Trail reconstruction itself (see `audit.rs`)
//! - Correction context routing (see `correct_context_tests.rs`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::audit::audit_trail;
use clio_store::{LocalDevKms, SqliteStore, Store};
use clio_types::{ItemKind, MemoryItem, SemanticCategory};
use serde_json::json;
use std::sync::Arc;

fn item(id: &str, bank: &str, context: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: "subj".into(),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::TaskSpec),
        episodic_type: None,
        epistemic_kind: clio_types::EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.9),
        admission_score: Some(0.88),
        source_ref: Some("turn-9".into()),
        snapshot: Some(json!({"deploy": "v1"})),
        gist: Some("deploys nightly".into()),
        valid_from: "2024-01-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2024-01-01T00:00:00Z".into(),
        tx_until: None,
        created_at: "2024-01-01T00:00:00Z".into(),
        updated_at: "2024-01-01T00:00:00Z".into(),
        context: Some(context.to_owned()),
    }
}

fn seeded(context: &str) -> (SqliteStore, String, String) {
    let store = SqliteStore::open_in_memory(Arc::new(LocalDevKms::new())).expect("store");
    let bank = format!("bank-audit-ctx-{}", std::process::id());
    let id = "ctx-item";
    store
        .create_memory_item(&item(id, &bank, context), "agent")
        .expect("seed item");
    (store, bank, id.to_owned())
}

#[test]
fn secret_shaped_context_is_scrubbed_in_the_view() {
    let (store, bank, id) = seeded("api_key: sk-test-1234567890");
    let view = audit_trail(&store, &bank, &id).expect("trail");
    let revision = view.revisions.first().expect("revision");
    assert_eq!(revision["context"], json!("api_key: ****7890"));
    let text = serde_json::to_string(&view).expect("view serializes");
    assert!(
        !text.contains("sk-test-1234567890"),
        "raw secret must not reach the trail view"
    );
    // The merged timeline carries the same scrubbed revision.
    let timeline_revision = view
        .timeline
        .iter()
        .find(|t| t["kind"] == "revision")
        .expect("timeline revision");
    assert_eq!(
        timeline_revision["entry"]["context"],
        json!("api_key: ****7890")
    );
    // Telemetry keeps presence/length/hash only: no raw context anywhere.
    let create = view
        .events
        .iter()
        .find(|e| e["operation"] == "create_memory_item")
        .expect("creation event");
    assert_eq!(create["detail"]["context_present"], json!(true));
    assert_eq!(
        create["detail"]["context_len"],
        json!("api_key: sk-test-1234567890".len())
    );
    assert!(
        !serde_json::to_string(create)
            .expect("event serializes")
            .contains("api_key")
    );
}

#[test]
fn non_secret_context_stays_readable_in_the_view() {
    let (store, bank, id) = seeded("team chat");
    let view = audit_trail(&store, &bank, &id).expect("trail");
    let revision = view.revisions.first().expect("revision");
    assert_eq!(revision["context"], json!("team chat"));
}
