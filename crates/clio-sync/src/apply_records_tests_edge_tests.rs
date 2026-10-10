// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the edge branches of the domain-record sync apply
//! effects: cipher-content no-op applies, record-level duplicate and tie
//! resolution against locally stored rows, divergence journaling, belief
//! id reuse rejects, malformed payload dead-letters, and the preference
//! erasure fail-closed path.
//!
//! ## Owns
//! - Edge-branch behavior coverage for `apply_records`
//!
//! ## Does not own
//! - Happy-path persona/belief apply tests (parent `apply_records_tests`)
//! - Item/assoc/triple apply tests (`apply_tests` / `apply_matrix_tests`)
//!
//! ## Boundary
//!
//! Tests here break when record apply edge semantics change; they never
//! change it.
//!
//! Keep this module focused on domain-record apply edge tests.

use super::*;
use crate::apply::apply_tests::{apply, fixture, receipt};
use crate::crypto::SyncCipher;
use crate::protocol::SyncContent;
use clio_store::{ComplianceStore, SubjectEraseSpec, SyncStore};

fn pref(key: &str, value: f64, as_of: &str) -> PersonaPreference {
    PersonaPreference {
        bank_id: "b-rec".into(),
        key: key.into(),
        value,
        trend: 0.1,
        confidence: None,
        source: "inferred".into(),
        source_ref: None,
        context: Some("c1".into()),
        admission_score: 0.9,
        as_of: as_of.into(),
    }
}

fn cipher_m(mid: &str, kind: &str, entity: &str) -> SyncMutation {
    SyncMutation {
        sync_mutation_id: mid.into(),
        bank: "b-rec".into(),
        entity_kind: kind.into(),
        entity_id: entity.into(),
        op: "upsert".into(),
        transaction_time: "2026-01-01T00:00:00Z".into(),
        device_id: "dev-a".into(),
        content: SyncContent::Cipher(SyncCipher {
            alg: "xchacha20poly1305".into(),
            key_id: None,
            nonce_b64: "AA==".into(),
            ciphertext_b64: "AA==".into(),
        }),
        content_hash: clio_store::sha256_hex("am-sync-hash".as_bytes(), "{}"),
        admission_receipt: None,
    }
}

/// Server-blind ciphertext rides the persona/belief apply matrix as a no-op
/// apply: the engine journals it without touching local durable rows.
#[test]
fn cipher_content_persona_and_belief_apply_without_effect() {
    let (store, _kms) = fixture();
    assert_eq!(
        apply(&store, None, &cipher_m("m-cp", "persona", "style")),
        ApplyOutcome::Applied
    );
    assert_eq!(
        apply(&store, None, &cipher_m("m-cb", "belief", "b-1")),
        ApplyOutcome::Applied
    );
    let doc = store.get_persona_document("b-rec").expect("persona doc");
    assert!(doc.stable.is_empty(), "{:?}", doc.stable);
    let beliefs = store.list_beliefs("b-rec").expect("beliefs");
    assert!(beliefs.is_empty(), "{beliefs:?}");
}

/// An equal-version apply of a locally seeded row under a fresh mutation id
/// journals the mutation (the journal self-heal: the durable row already
/// exists, so the only missing piece is the pull-source journal row), with
/// no conflict journaled.
#[test]
fn persona_stable_equal_version_from_stored_row_applies_and_journals() {
    let (store, _kms) = fixture();
    let seed = stable("style", "terse", "2026-01-02", 0.9);
    store.put_stable_entry(&seed, "seed").expect("seed");
    let m = mut_for(
        "m-eq",
        "persona",
        "style",
        "2026-01-02T00:00:00Z",
        payload("stable", &seed),
    );
    assert_eq!(apply(&store, None, &m), ApplyOutcome::Applied);
    assert!(
        store
            .journal_get("b-rec", "m-eq")
            .expect("journal")
            .is_some(),
        "the equal-state apply must be journaled so pull can relay it"
    );
    assert_eq!(store.conflict_count("b-rec").expect("conflicts"), 0);
    let doc = store.get_persona_document("b-rec").expect("doc");
    assert_eq!(doc.stable.len(), 1);
    assert_eq!(doc.stable[0].value, "terse");
}

/// Same version slot with differing content: the documented tie rule applies.
/// A lower admission score loses (conflict journaled); a higher score wins.
#[test]
fn persona_stable_tie_resolves_by_admission_score() {
    let (store, _kms) = fixture();
    let seed = stable("style", "terse", "2026-01-02", 0.9);
    store.put_stable_entry(&seed, "seed").expect("seed");
    let differing = stable("style", "brief", "2026-01-02", 0.5);
    let loser = receipt(
        &mut_for(
            "m-tie-loser",
            "persona",
            "style",
            "2026-01-02T00:00:00Z",
            payload("stable", &differing),
        ),
        0.4,
    );
    assert_eq!(apply(&store, None, &loser), ApplyOutcome::ConflictLoser);
    assert_eq!(store.conflict_count("b-rec").expect("conflicts"), 1);
    let winner = receipt(
        &mut_for(
            "m-tie-winner",
            "persona",
            "style",
            "2026-01-02T00:00:00Z",
            payload("stable", &differing),
        ),
        0.95,
    );
    assert_eq!(apply(&store, None, &winner), ApplyOutcome::Applied);
    let doc = store.get_persona_document("b-rec").expect("doc");
    assert_eq!(doc.stable[0].value, "brief");
}

/// Preference applies: identical re-delivery of a stored row duplicates, an
/// older `as_of` loses, and an equal-slot tie resolves by admission score.
#[test]
fn persona_preference_duplicate_older_and_tie_outcomes() {
    let (store, _kms) = fixture();
    let seed = pref("brevity", 0.4, "2026-01-02");
    store.put_preference_state(&seed, "seed").expect("seed");
    let dup = mut_for(
        "m-pd",
        "persona",
        "brevity",
        "2026-01-02T00:00:00Z",
        payload("preference", &seed),
    );
    // Equal state under a fresh mutation id journals the mutation (journal
    // self-heal) instead of dropping it.
    assert_eq!(apply(&store, None, &dup), ApplyOutcome::Applied);
    let mut older = seed.clone();
    older.as_of = "2026-01-01".into();
    assert_eq!(
        apply(
            &store,
            None,
            &mut_for(
                "m-po",
                "persona",
                "brevity",
                "2026-01-01T00:00:00Z",
                payload("preference", &older)
            )
        ),
        ApplyOutcome::ConflictLoser
    );
    let mut tied = seed;
    tied.value = 0.5;
    let tie_payload = payload("preference", &tied);
    assert_eq!(
        apply(
            &store,
            None,
            &receipt(
                &mut_for(
                    "m-ptl",
                    "persona",
                    "brevity",
                    "2026-01-02T00:00:00Z",
                    tie_payload.clone()
                ),
                0.4
            )
        ),
        ApplyOutcome::ConflictLoser
    );
    assert_eq!(
        apply(
            &store,
            None,
            &receipt(
                &mut_for(
                    "m-ptw",
                    "persona",
                    "brevity",
                    "2026-01-02T00:00:00Z",
                    tie_payload
                ),
                0.95
            )
        ),
        ApplyOutcome::Applied
    );
    let got = store
        .get_preference("b-rec", "brevity")
        .expect("pref")
        .expect("row");
    // `value` is an `f64` round-tripped bit-exactly; compare with an explicit
    // tolerance instead of strict float equality.
    assert!((got.value - 0.5).abs() < 1e-12, "value = {}", got.value);
    assert_eq!(got.context.as_deref(), Some("c1"));
}

/// A persona erasure tombstone fail-closes the preference apply effect: after
/// a verified erasure purged the bank's persona context (the persona purge is
/// bank-scoped), a delayed persona mutation dead-letters instead of
/// resurrecting the purged context.
#[test]
fn persona_preference_erased_bank_dead_letters() {
    let (store, _kms) = fixture();
    // Seed persona content so the verified erase below purges it and writes
    // the bank-level tombstone (a bank with nothing purged stays writable).
    let seed = pref("brevity", 0.4, "2026-01-01");
    store.put_preference_state(&seed, "seed").expect("seed");
    store
        .erase_subject(&SubjectEraseSpec {
            bank_id: "b-rec",
            subject_id: "persona-erase-subject",
            request_id: "req-erased",
            legal_basis: "GDPR_ART_17",
            reason_code: "COMPLIANCE_CRYPTO_SHRED",
            deleted_at: "2026-01-02T00:00:00Z",
            salt: b"salt",
        })
        .expect("erase");
    let m = mut_for(
        "m-psp",
        "persona",
        "brevity",
        "2026-01-01T00:00:00Z",
        payload("preference", &pref("brevity", 0.4, "2026-01-01")),
    );
    assert_eq!(
        apply(&store, None, &m),
        ApplyOutcome::DeadLettered {
            reason_code: "erased_subject".into()
        }
    );
}

/// Malformed persona payloads fail closed: an unparsable entry and an entity
/// id that is not the attr key both quarantine as `malformed`.
#[test]
fn persona_malformed_payload_variants_dead_letter() {
    let (store, _kms) = fixture();
    let no_entry = mut_for(
        "m-mp1",
        "persona",
        "style",
        "2026-01-01T00:00:00Z",
        json!({ "record": "stable" }),
    );
    assert_eq!(
        apply(&store, None, &no_entry),
        ApplyOutcome::DeadLettered {
            reason_code: "malformed".into()
        }
    );
    let mismatch = mut_for(
        "m-mp2",
        "persona",
        "not-style",
        "2026-01-01T00:00:00Z",
        payload("stable", &stable("style", "terse", "2026-01-01", 0.5)),
    );
    assert_eq!(
        apply(&store, None, &mismatch),
        ApplyOutcome::DeadLettered {
            reason_code: "malformed".into()
        }
    );
    let bad_pref = mut_for(
        "m-mp3",
        "persona",
        "brevity",
        "2026-01-01T00:00:00Z",
        json!({ "record": "preference", "entry": { "key": "brevity" } }),
    );
    assert_eq!(
        apply(&store, None, &bad_pref),
        ApplyOutcome::DeadLettered {
            reason_code: "malformed".into()
        }
    );
}

/// A belief whose wire bank differs from the mutation envelope is rejected,
/// and unparsable belief content quarantines as `malformed`.
#[test]
fn belief_bank_mismatch_and_malformed_content_dead_letter() {
    let (store, _kms) = fixture();
    let mut foreign = belief("b-1", "s1", 1);
    foreign.bank_id = "b-other".into();
    let m = mut_for(
        "m-bf",
        "belief",
        "b-1",
        "2026-01-01T00:00:00Z",
        serde_json::to_value(&foreign).expect("serialize"),
    );
    assert_eq!(
        apply(&store, None, &m),
        ApplyOutcome::DeadLettered {
            reason_code: "unauthorized_bank".into()
        }
    );
    let garbage = mut_for(
        "m-bg",
        "belief",
        "b-1",
        "2026-01-01T00:00:00Z",
        json!({ "id": "b-x" }),
    );
    assert_eq!(
        apply(&store, None, &garbage),
        ApplyOutcome::DeadLettered {
            reason_code: "malformed".into()
        }
    );
}

/// A belief id that already belongs to another proposition is a usage error:
/// the mutation dead-letters instead of silently re-pointing the id.
#[test]
fn belief_id_reuse_across_propositions_dead_letters() {
    let (store, _kms) = fixture();
    let seed = belief("b-1", "s1", 1);
    store.create_belief(&seed, "seed").expect("seed");
    let mut reuse = belief("b-1", "s1", 1);
    reuse.proposition = "user prefers verbose replies".into();
    let m = mut_for(
        "m-reuse",
        "belief",
        "b-1",
        "2026-01-01T00:00:00Z",
        serde_json::to_value(&reuse).expect("serialize"),
    );
    assert_eq!(
        apply(&store, None, &m),
        ApplyOutcome::DeadLettered {
            reason_code: "malformed".into()
        }
    );
}

/// A trajectory the store already supersedes duplicates; a diverging
/// trajectory keeps the local row and journals the incoming mutation.
#[test]
fn belief_local_ahead_applies_and_diverged_records_conflict() {
    let (store, _kms) = fixture();
    let e1 = ConfidenceEntry::new("2026-01-01", 0.6, "ev-1", SourceType::UserStated).expect("e1");
    let e2 = ConfidenceEntry::new("2026-01-02", 0.7, "ev-2", SourceType::UserStated).expect("e2");
    let e3 = ConfidenceEntry::new("2026-01-02", 0.9, "ev-3", SourceType::UserStated).expect("e3");
    let mut seed = belief("b-1", "s1", 0);
    seed.confidence_history = vec![e1.clone(), e2];
    store.create_belief(&seed, "seed").expect("seed");
    let mut shorter = seed.clone();
    shorter.confidence_history = vec![e1.clone()];
    assert_eq!(
        apply(
            &store,
            None,
            &mut_for(
                "m-ahead",
                "belief",
                "b-1",
                "2026-01-02T00:00:00Z",
                serde_json::to_value(&shorter).expect("serialize")
            )
        ),
        // LocalAhead under a fresh mutation id journals the mutation (journal
        // self-heal) so the pull path can relay the stored row.
        ApplyOutcome::Applied
    );
    let mut diverged = seed;
    diverged.confidence_history = vec![e1, e3];
    let mid = "m-div";
    assert_eq!(
        apply(
            &store,
            None,
            &mut_for(
                mid,
                "belief",
                "b-1",
                "2026-01-03T00:00:00Z",
                serde_json::to_value(&diverged).expect("serialize")
            )
        ),
        ApplyOutcome::ConflictLoser
    );
    let conflicts = store.conflict_list("b-rec", 10).expect("conflicts");
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].loser_mutation_id, mid);
    assert_eq!(conflicts[0].winner_mutation_id, "local:b-1");
    assert_eq!(conflicts[0].winner_device_id, "node-1");
    let got = store
        .get_belief("b-rec", "b-1")
        .expect("belief")
        .expect("row");
    assert_eq!(got.confidence_history.len(), 2);
    // `confidence` is an `f64` round-tripped bit-exactly; compare with an
    // explicit tolerance instead of strict float equality.
    assert!(
        (got.confidence_history[1].confidence - 0.7).abs() < 1e-12,
        "confidence = {}",
        got.confidence_history[1].confidence
    );
}
