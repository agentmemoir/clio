// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the remaining apply-matrix branches: item
//! invalidate/discard dedupe, association upsert/reinforce/prune, triple
//! invalidate, and the journal cipher/kms row paths.
//!
//! ## Owns
//! - Behavior coverage for the metadata + assoc + triple apply branches
//!
//! ## Does not own
//! - Core dedupe/LWW/DLQ paths (`apply_tests`)
//! - Transport integration (`server_tests` / `client_tests`)
//!
//! ## Boundary
//!
//! Tests here break when apply semantics change; they never change them.
//!
//! Keep this module focused on the apply-matrix tail tests.

use crate::apply::apply_tests::{apply, assoc, edge_value, env, fixture, item, upsert};
use crate::apply::{ApplyEngine, ApplyOutcome, quarantine};
use crate::crypto::SyncCipherKey;
use crate::protocol::{
    ENTITY_KIND_ITEM, ENTITY_KIND_TRIPLE, OP_ARCHIVE, OP_DISCARD, OP_INVALIDATE, OP_PRUNE,
    OP_REINFORCE, OP_UPSERT, SyncContent, SyncMutation,
};
use clio_store::DeadLetterStatus;
use clio_store::Store as _;
use clio_store::SyncStore as _;
use clio_types::{EpistemicKind, ItemKind, TripleRecord};
use serde_json::json;

#[test]
fn invalidate_missing_noop_existing_closes_and_discard_dedupes() {
    let (store, _kms) = fixture();
    let bank = "b-life";
    let miss = env(
        "m-miss",
        bank,
        ENTITY_KIND_ITEM,
        "nope",
        OP_INVALIDATE,
        "2026-01-01T00:00:00Z",
        "dev-a",
        json!({}),
    );
    assert_eq!(apply(&store, None, &miss), ApplyOutcome::Applied);
    store
        .create_memory_item(
            &item(bank, "i-1", "s1", "2026-01-01T00:00:00Z", "g"),
            "actor",
        )
        .unwrap();
    let close = env(
        "m-close",
        bank,
        ENTITY_KIND_ITEM,
        "i-1",
        OP_INVALIDATE,
        "2026-01-02T00:00:00Z",
        "dev-a",
        json!({}),
    );
    assert_eq!(apply(&store, None, &close), ApplyOutcome::Applied);
    let got = store.get_memory_item(bank, "i-1").unwrap().expect("item");
    assert!(got.tx_until.is_some());
    assert!(got.valid_until.is_some());
    let first = env(
        "m-d1",
        bank,
        ENTITY_KIND_ITEM,
        "i-1",
        OP_DISCARD,
        "2026-01-03T00:00:00Z",
        "dev-a",
        json!({ "r": 1 }),
    );
    assert_eq!(apply(&store, None, &first), ApplyOutcome::Applied);
    // A repeat discard reads as "missing" (identity lookup excludes discarded
    // rows), so the engine takes the Applied no-op path, not Duplicate.
    let mut again = first;
    again.sync_mutation_id = "m-d2".into();
    again.content_hash = "other-hash".into();
    assert_eq!(apply(&store, None, &again), ApplyOutcome::Applied);
    assert!(store.get_item_identity(bank, "i-1").unwrap().is_none());
}

#[test]
fn assoc_upsert_reinforce_lww_and_prune_paths() {
    let (store, kms) = fixture();
    let bank = "b-assoc";
    for id in ["i-a", "i-b"] {
        store
            .create_memory_item(&item(bank, id, "s1", "2026-01-01T00:00:00Z", id), "actor")
            .unwrap();
    }
    let up = |mid: &str, stamp: &str, dev: &str| {
        assoc(
            mid,
            bank,
            "e-1",
            OP_UPSERT,
            stamp,
            dev,
            edge_value(bank, "e-1", "i-a", "i-b", stamp),
        )
    };
    assert_eq!(
        apply(
            &store,
            Some(kms.as_ref()),
            &up("m-e1", "2026-01-01T00:00:00Z", "dev-a")
        ),
        ApplyOutcome::Applied
    );
    assert!(store.get_assoc_edge(bank, "e-1").unwrap().is_some());
    let re = assoc(
        "m-e2",
        bank,
        "e-1",
        OP_REINFORCE,
        "2026-01-02T00:00:00Z",
        "dev-b",
        edge_value(bank, "e-1", "i-a", "i-b", "2026-01-02T00:00:00Z"),
    );
    assert_eq!(
        apply(&store, Some(kms.as_ref()), &re),
        ApplyOutcome::Applied
    );
    let stale = up("m-e3", "2026-01-01T06:00:00Z", "dev-late");
    assert_eq!(
        apply(&store, Some(kms.as_ref()), &stale),
        ApplyOutcome::ConflictLoser
    );
    assert_eq!(store.conflict_count(bank).unwrap(), 1);
    let prune = assoc(
        "m-p",
        bank,
        "e-1",
        OP_PRUNE,
        "2026-01-03T00:00:00Z",
        "dev-a",
        edge_value(bank, "e-1", "i-a", "i-b", "2026-01-03T00:00:00Z"),
    );
    assert_eq!(
        apply(&store, Some(kms.as_ref()), &prune),
        ApplyOutcome::Applied
    );
    assert!(store.get_assoc_edge(bank, "e-1").unwrap().is_none());
}

#[test]
fn assoc_rejects_missing_endpoint_bank_mismatch_and_bad_content() {
    let (store, _kms) = fixture();
    let bank = "b-assoc2";
    let ghost = assoc(
        "m-e1",
        bank,
        "e-x",
        OP_UPSERT,
        "2026-01-01T00:00:00Z",
        "dev-a",
        edge_value(bank, "e-x", "ghost-a", "ghost-b", "2026-01-01T00:00:00Z"),
    );
    assert_eq!(
        apply(&store, None, &ghost),
        ApplyOutcome::DeadLettered {
            reason_code: "malformed".into()
        }
    );
    let cross = assoc(
        "m-e2",
        "bank-wire",
        "e-y",
        OP_UPSERT,
        "2026-01-01T00:00:00Z",
        "dev-a",
        edge_value("bank-edge", "e-y", "i-a", "i-b", "2026-01-01T00:00:00Z"),
    );
    assert_eq!(
        apply(&store, None, &cross),
        ApplyOutcome::DeadLettered {
            reason_code: "unauthorized_bank".into()
        }
    );
    let junk = assoc(
        "m-e3",
        bank,
        "e-z",
        OP_UPSERT,
        "2026-01-01T00:00:00Z",
        "dev-a",
        json!({ "nonsense": true }),
    );
    // Bad assoc content is a permanent per-mutation reject: it quarantines as
    // `malformed` so the cursor freezes on the id instead of a retryable 500.
    assert_eq!(
        apply(&store, None, &junk),
        ApplyOutcome::DeadLettered {
            reason_code: "malformed".into()
        }
    );
}

#[test]
fn triple_invalidate_closes_local_edge_and_reclose_is_noop() {
    let (store, _kms) = fixture();
    let bank = "b-triple";
    let mut carrier = item(bank, "c-1", "s1", "2026-01-01T00:00:00Z", "carrier");
    carrier.kind = ItemKind::Episodic;
    carrier.category = None;
    carrier.episodic_type = Some(clio_types::EpisodicType::Triple);
    // commit_triple_add creates the gated carrier item itself.
    let rec = TripleRecord {
        context: None,
        id: "e-t1".into(),
        bank_id: bank.into(),
        item_id: carrier.id.clone(),
        subject: "Alice".into(),
        predicate: "lives_in".into(),
        object: "Oslo".into(),
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.9),
        valid_from: "2026-01-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2026-01-01T00:00:00Z".into(),
        tx_until: None,
        snapshot_ref: None,
        gist_ref: None,
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: None,
    };
    store
        .commit_triple_add(&carrier, &rec, true, "actor")
        .expect("triple add");
    let close = env(
        "m-t1",
        bank,
        ENTITY_KIND_TRIPLE,
        "e-t1",
        OP_INVALIDATE,
        "2026-01-02T00:00:00Z",
        "dev-a",
        json!({ "subject": "Alice", "predicate": "lives_in", "object": "Oslo" }),
    );
    assert_eq!(apply(&store, None, &close), ApplyOutcome::Applied);
    let got = store.get_triple(bank, "e-t1").unwrap().expect("triple");
    assert!(got.tx_until.is_some());
    assert!(got.valid_until.is_some());
    // Nothing open remains for this identity: SQLite reports already_ended, so
    // the engine treats a re-close as an Applied no-op (the Duplicate arm only
    // fires for identities the store never knew).
    let reclose = env(
        "m-t2",
        bank,
        ENTITY_KIND_TRIPLE,
        "e-t1",
        OP_INVALIDATE,
        "2026-01-03T00:00:00Z",
        "dev-a",
        json!({ "subject": "Alice", "predicate": "lives_in", "object": "Paris" }),
    );
    assert_eq!(apply(&store, None, &reclose), ApplyOutcome::Applied);
    let bad = env(
        "m-t3",
        bank,
        ENTITY_KIND_TRIPLE,
        "e-t1",
        OP_INVALIDATE,
        "2026-01-01T00:00:00Z",
        "dev-a",
        json!({ "subject": "", "predicate": "" }),
    );
    assert_eq!(
        apply(&store, None, &bad),
        ApplyOutcome::DeadLettered {
            reason_code: "malformed".into()
        }
    );
}

#[test]
fn cipher_and_keyless_journal_paths() {
    let (store, kms) = fixture();
    let key = SyncCipherKey::from_secret(&crate::credential_fixtures::secret("t24-cipher"));
    let sealed = SyncContent::Cipher(key.seal(br"{}").expect("seal"));
    let mut m = upsert("m-c1", "b-cipher", "2026-01-01T00:00:00Z", "dev-a", "g");
    m.content = sealed.clone();
    assert_eq!(apply(&store, Some(kms.as_ref()), &m), ApplyOutcome::Applied);
    assert!(store.get_memory_item("b-cipher", "i-1").unwrap().is_none());
    let row = store
        .journal_get("b-cipher", "m-c1")
        .unwrap()
        .expect("journaled");
    assert!(row.client_encrypted);
    assert!(row.content_sealed.is_some());
    let assoc = SyncMutation {
        content: sealed.clone(),
        ..assoc(
            "m-c2",
            "b-cipher",
            "e-1",
            OP_UPSERT,
            "2026-01-01T00:00:00Z",
            "dev-a",
            json!({}),
        )
    };
    assert_eq!(
        apply(&store, Some(kms.as_ref()), &assoc),
        ApplyOutcome::Applied
    );
    let triple = SyncMutation {
        content: sealed,
        ..env(
            "m-c3",
            "b-cipher",
            ENTITY_KIND_TRIPLE,
            "e-1",
            OP_INVALIDATE,
            "2026-01-01T00:00:00Z",
            "dev-a",
            json!({}),
        )
    };
    assert_eq!(
        apply(&store, Some(kms.as_ref()), &triple),
        ApplyOutcome::Applied
    );
    // A pure-client engine (kms None) journals ids/hashes only.
    let plain = upsert("m-1", "b-nokms", "2026-01-01T00:00:00Z", "dev-a", "g");
    assert_eq!(apply(&store, None, &plain), ApplyOutcome::Applied);
    let row = store
        .journal_get("b-nokms", "m-1")
        .unwrap()
        .expect("journaled");
    assert!(row.content_sealed.is_none());
    assert!(!row.client_encrypted);
}

#[test]
fn quarantine_helper_opens_an_open_dead_letter_row() {
    let (store, _kms) = fixture();
    let m = upsert("m-q", "b-q", "2026-01-01T00:00:00Z", "dev-a", "g");
    let engine = ApplyEngine {
        store: store.as_ref(),
        sync: store.as_ref(),
        device_id: "node-1".into(),
        kms: None,
    };
    let envelope = serde_json::to_string(&m).expect("envelope");
    let out =
        quarantine(&engine, &m.bank, &m, &envelope, "malformed", "no key").expect("quarantine");
    assert_eq!(
        out,
        ApplyOutcome::DeadLettered {
            reason_code: "malformed".into()
        }
    );
    assert_eq!(
        store
            .dead_letter_count("b-q", DeadLetterStatus::Open)
            .unwrap(),
        1
    );
}

#[test]
fn unsupported_kind_op_and_bad_item_content_dead_letter() {
    let (store, _kms) = fixture();
    let kind = env(
        "m-unk",
        "b-uns",
        "widget",
        "i-1",
        OP_UPSERT,
        "2026-01-01T00:00:00Z",
        "dev-a",
        json!({}),
    );
    assert_eq!(
        apply(&store, None, &kind),
        ApplyOutcome::DeadLettered {
            reason_code: "malformed".into()
        }
    );
    let m = env(
        "m-arc",
        "b-uns",
        ENTITY_KIND_ITEM,
        "i-1",
        OP_ARCHIVE,
        "2026-01-01T00:00:00Z",
        "dev-a",
        json!({}),
    );
    assert_eq!(
        apply(&store, None, &m),
        ApplyOutcome::DeadLettered {
            reason_code: "unsupported_op".into()
        }
    );
    let bad = env(
        "m-badi",
        "b-uns",
        ENTITY_KIND_ITEM,
        "i-1",
        OP_UPSERT,
        "2026-01-01T00:00:00Z",
        "dev-a",
        json!({ "nonsense": true }),
    );
    // Content-parse failures quarantine as `malformed` (a permanent
    // per-mutation reject), the same as envelope validation failures.
    assert_eq!(
        apply(&store, None, &bad),
        ApplyOutcome::DeadLettered {
            reason_code: "malformed".into()
        }
    );
    let open = store
        .dead_letter_list("b-uns", DeadLetterStatus::Open, 10)
        .unwrap();
    assert!(open.iter().any(|r| r.reason_code == "unsupported_op"));
}
