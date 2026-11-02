// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the assoc-edge and triple apply effects in isolation:
//! the edge upsert, the last-writer-wins loser, the prune, the triple
//! invalidate, and the sealed-content short circuit.
//!
//! ## Owns
//! - Behavior coverage for the edge and triple effects (`apply_edges`)
//!
//! ## Does not own
//! - The apply order that calls these effects (`apply_matrix_tests`)
//! - The core dedupe, LWW, and dead-letter paths (`apply_tests`)
//!
//! ## Boundary
//!
//! Tests here break when edge or triple apply behavior changes; they never
//! change it.
//!
//! Keep this module focused on the edge and triple effects.

use crate::apply::ApplyOutcome;
use crate::apply::apply_tests::{apply, assoc, edge_value, env, fixture, item};
use crate::crypto::SyncCipherKey;
use crate::protocol::{
    ENTITY_KIND_ASSOC, ENTITY_KIND_TRIPLE, OP_INVALIDATE, OP_PRUNE, OP_UPSERT, SyncContent,
    SyncMutation,
};
use clio_store::Store as _;
use clio_store::SyncStore as _;
use clio_types::{EpistemicKind, ItemKind, TripleRecord};
use serde_json::json;

fn seed_endpoints(store: &clio_store::SqliteStore, bank: &str) {
    for id in ["n-1", "n-2"] {
        store
            .create_memory_item(&item(bank, id, "s1", "2026-02-01T00:00:00Z", id), "actor")
            .unwrap();
    }
}

fn triple_record(bank: &str, id: &str, item_id: &str) -> TripleRecord {
    TripleRecord {
        context: None,
        id: id.into(),
        bank_id: bank.into(),
        item_id: item_id.into(),
        subject: "Bob".into(),
        predicate: "works_at".into(),
        object: "Acme".into(),
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.8),
        valid_from: "2026-02-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2026-02-01T00:00:00Z".into(),
        tx_until: None,
        snapshot_ref: None,
        gist_ref: None,
        created_at: "2026-02-01T00:00:00Z".into(),
        updated_at: None,
    }
}

#[test]
fn assoc_upsert_then_stale_writer_loses_then_prune_removes() {
    let (store, kms) = fixture();
    let bank = "b-edge-lww";
    seed_endpoints(&store, bank);
    let up = env(
        "m-e1",
        bank,
        ENTITY_KIND_ASSOC,
        "edge-1",
        OP_UPSERT,
        "2026-02-02T00:00:00Z",
        "dev-a",
        edge_value(bank, "edge-1", "n-1", "n-2", "2026-02-02T00:00:00Z"),
    );
    assert_eq!(
        apply(&store, Some(kms.as_ref()), &up),
        ApplyOutcome::Applied
    );
    let stored = store
        .get_assoc_edge(bank, "edge-1")
        .unwrap()
        .expect("edge stored");
    assert_eq!(stored.last_updated_at, "2026-02-02T00:00:00Z");

    let stale = env(
        "m-e2",
        bank,
        ENTITY_KIND_ASSOC,
        "edge-1",
        OP_UPSERT,
        "2026-02-01T12:00:00Z",
        "dev-late",
        edge_value(bank, "edge-1", "n-1", "n-2", "2026-02-01T12:00:00Z"),
    );
    assert_eq!(
        apply(&store, Some(kms.as_ref()), &stale),
        ApplyOutcome::ConflictLoser
    );
    assert_eq!(store.conflict_count(bank).unwrap(), 1);

    let newer = env(
        "m-e4",
        bank,
        ENTITY_KIND_ASSOC,
        "edge-1",
        OP_UPSERT,
        "2026-02-02T06:00:00Z",
        "dev-a",
        edge_value(bank, "edge-1", "n-1", "n-2", "2026-02-02T06:00:00Z"),
    );
    assert_eq!(
        apply(&store, Some(kms.as_ref()), &newer),
        ApplyOutcome::Applied
    );
    let updated = store
        .get_assoc_edge(bank, "edge-1")
        .unwrap()
        .expect("edge updated");
    assert_eq!(updated.last_updated_at, "2026-02-02T06:00:00Z");

    let prune = env(
        "m-e3",
        bank,
        ENTITY_KIND_ASSOC,
        "edge-1",
        OP_PRUNE,
        "2026-02-03T00:00:00Z",
        "dev-a",
        edge_value(bank, "edge-1", "n-1", "n-2", "2026-02-03T00:00:00Z"),
    );
    assert_eq!(
        apply(&store, Some(kms.as_ref()), &prune),
        ApplyOutcome::Applied
    );
    assert!(store.get_assoc_edge(bank, "edge-1").unwrap().is_none());
}

#[test]
fn assoc_blank_identity_dead_letters_as_unauthorized_bank() {
    let (store, _kms) = fixture();
    let cross = assoc(
        "m-x1",
        "bank-wire",
        "edge-x",
        OP_UPSERT,
        "2026-02-02T00:00:00Z",
        "dev-a",
        edge_value("bank-edge", "edge-x", "n-1", "n-2", "2026-02-02T00:00:00Z"),
    );
    assert_eq!(
        apply(&store, None, &cross),
        ApplyOutcome::DeadLettered {
            reason_code: "unauthorized_bank".into()
        }
    );
}

#[test]
fn assoc_unparseable_content_dead_letters_as_malformed() {
    let (store, _kms) = fixture();
    let junk = env(
        "m-j1",
        "b-edge-bad",
        ENTITY_KIND_ASSOC,
        "edge-j",
        OP_UPSERT,
        "2026-02-02T00:00:00Z",
        "dev-a",
        json!({ "nonsense": true }),
    );
    assert_eq!(
        apply(&store, None, &junk),
        ApplyOutcome::DeadLettered {
            reason_code: "malformed".into()
        }
    );
}

#[test]
fn assoc_missing_endpoint_dead_letters_as_malformed() {
    let (store, _kms) = fixture();
    let ghost = env(
        "m-g1",
        "b-edge-ghost",
        ENTITY_KIND_ASSOC,
        "edge-g",
        OP_UPSERT,
        "2026-02-02T00:00:00Z",
        "dev-a",
        edge_value(
            "b-edge-ghost",
            "edge-g",
            "n-ghost-a",
            "n-ghost-b",
            "2026-02-02T00:00:00Z",
        ),
    );
    assert_eq!(
        apply(&store, None, &ghost),
        ApplyOutcome::DeadLettered {
            reason_code: "malformed".into()
        }
    );
}

#[test]
fn triple_invalidate_closes_open_row_and_blank_identity_dead_letters() {
    let (store, _kms) = fixture();
    let bank = "b-edge-triple";
    let mut carrier = item(bank, "c-9", "s1", "2026-02-01T00:00:00Z", "carrier");
    carrier.kind = ItemKind::Episodic;
    carrier.category = None;
    carrier.episodic_type = Some(clio_types::EpisodicType::Triple);
    let rec = triple_record(bank, "t-1", &carrier.id);
    store
        .commit_triple_add(&carrier, &rec, true, "actor")
        .expect("triple add");

    let close = env(
        "m-t1",
        bank,
        ENTITY_KIND_TRIPLE,
        "t-1",
        OP_INVALIDATE,
        "2026-02-05T00:00:00Z",
        "dev-a",
        json!({ "subject": "Bob", "predicate": "works_at", "object": "Acme" }),
    );
    assert_eq!(apply(&store, None, &close), ApplyOutcome::Applied);
    let got = store.get_triple(bank, "t-1").unwrap().expect("triple");
    assert_eq!(got.tx_until.as_deref(), Some("2026-02-05T00:00:00Z"));
    assert!(got.valid_until.is_some());

    let no_identity = env(
        "m-t2",
        bank,
        ENTITY_KIND_TRIPLE,
        "t-1",
        OP_INVALIDATE,
        "2026-02-06T00:00:00Z",
        "dev-a",
        json!({ "subject": "", "predicate": "works_at" }),
    );
    assert_eq!(
        apply(&store, None, &no_identity),
        ApplyOutcome::DeadLettered {
            reason_code: "malformed".into()
        }
    );
}

#[test]
fn sealed_content_short_circuits_both_effects() {
    let (store, kms) = fixture();
    let bank = "b-edge-sealed";
    let key = SyncCipherKey::from_secret(&crate::credential_fixtures::secret("edges-sealed"));
    let seed = env(
        "m-s0",
        bank,
        ENTITY_KIND_ASSOC,
        "edge-s",
        OP_UPSERT,
        "2026-02-02T00:00:00Z",
        "dev-a",
        json!({}),
    );
    let sealed = SyncContent::Cipher(
        key.seal(br"{}", &seed.execution_view().aad())
            .expect("seal"),
    );
    let assoc_m = SyncMutation {
        content: sealed.clone(),
        ..seed
    };
    assert_eq!(
        apply(&store, Some(kms.as_ref()), &assoc_m),
        ApplyOutcome::Applied
    );
    assert!(store.get_assoc_edge(bank, "edge-s").unwrap().is_none());

    let triple_seed = env(
        "m-s1",
        bank,
        ENTITY_KIND_TRIPLE,
        "t-s",
        OP_INVALIDATE,
        "2026-02-02T00:00:00Z",
        "dev-a",
        json!({ "subject": "Bob", "predicate": "works_at" }),
    );
    let triple_m = SyncMutation {
        content: sealed,
        ..triple_seed
    };
    assert_eq!(
        apply(&store, Some(kms.as_ref()), &triple_m),
        ApplyOutcome::Applied
    );
}
