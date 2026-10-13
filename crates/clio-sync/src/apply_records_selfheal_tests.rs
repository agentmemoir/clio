// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the persona/belief apply remedies: the journal
//! self-heal on equal-state persona redeliveries and belief `LocalAhead`
//! merges, the context-only belief correction that stays journaled, the
//! merge-decides-first ordering that keeps a diverged mutation from touching
//! the stored context, and the persona payload bank-mismatch fail-closed
//! check.
//!
//! ## Owns
//! - Regression coverage for persona/belief journal self-heal, belief
//!   correction ordering, and persona bank-mismatch dead-letters
//!
//! ## Does not own
//! - The happy-path persona/belief apply tests (`apply_records_tests`)
//! - History-record self-heal coverage (`apply_history_selfheal_tests`)
//!
//! ## Boundary
//!
//! Tests here break when record apply self-heal semantics change; they never
//! change it.
//!
//! Keep this module focused on domain-record self-heal tests.

use crate::apply::ApplyOutcome;
use crate::apply::apply_tests::{apply, fixture};
use crate::protocol::{SyncContent, SyncMutation};
use clio_store::{BeliefStore, PersonaStore, SyncStore};
use clio_types::{
    BeliefObject, ConfidenceEntry, PersonaPreference, PersonaStableEntry, SourceType,
};
use serde_json::{Value, json};

fn mut_for(mid: &str, kind: &str, entity: &str, stamp: &str, content: Value) -> SyncMutation {
    let content_hash = clio_store::sha256_hex(
        crate::keyed_roles::PUBLISHED_DIGEST_SALT,
        &serde_json::to_string(&content).unwrap_or_default(),
    );
    SyncMutation {
        sync_mutation_id: mid.into(),
        bank: "b-rec".into(),
        entity_kind: kind.into(),
        entity_id: entity.into(),
        op: "upsert".into(),
        transaction_time: stamp.into(),
        device_id: "dev-a".into(),
        content: SyncContent::Plain(content),
        content_hash,
        admission_receipt: None,
    }
}

fn stable_entry(as_of: &str, with_context: bool) -> PersonaStableEntry {
    PersonaStableEntry {
        bank_id: "b-rec".into(),
        key: "style".into(),
        value: "terse".into(),
        confidence: None,
        source: "explicit".into(),
        source_ref: Some("ev".into()),
        context: with_context.then(|| "why recorded".to_owned()),
        admission_score: 0.5,
        as_of: as_of.into(),
        valid_until: None,
    }
}

fn stable_payload(entry: &PersonaStableEntry) -> Value {
    json!({
        "record": "stable",
        "entry": serde_json::to_value(entry).expect("serialize entry"),
    })
}

/// Journal self-heal: an equal-state persona stable redelivery under a
/// fresh, never-journaled mutation id returns Applied and journals the
/// mutation (the context-absent variant keeps the payload hashes distinct so
/// the content guard misses).
#[test]
fn persona_stable_equal_version_fresh_id_applies_and_journals() {
    let (store, _kms) = fixture();
    let first = mut_for(
        "m-pbase",
        "persona",
        "style",
        "2026-01-01T00:00:00Z",
        stable_payload(&stable_entry("2026-01-02", true)),
    );
    assert_eq!(apply(&store, None, &first), ApplyOutcome::Applied);
    // Equal as_of, identical record fields, context omitted on the wire: the
    // stored context is preserved, the payload hash differs, and the
    // equal-state branch self-heals the journal.
    let dup = mut_for(
        "m-pdup",
        "persona",
        "style",
        "2026-01-02T00:00:00Z",
        stable_payload(&stable_entry("2026-01-02", false)),
    );
    assert_eq!(apply(&store, None, &dup), ApplyOutcome::Applied);
    assert!(store.journal_get("b-rec", "m-pdup").unwrap().is_some());
    assert_eq!(store.conflict_count("b-rec").unwrap(), 0);
    let doc = store.get_persona_document("b-rec").expect("doc");
    assert_eq!(doc.stable[0].value, "terse");
    assert_eq!(doc.stable[0].context.as_deref(), Some("why recorded"));
}

/// Journal self-heal: a same-slot preference redelivery under a fresh,
/// never-journaled mutation id returns Applied and journals the mutation.
#[test]
fn persona_pref_equal_version_fresh_id_applies_and_journals() {
    let (store, _kms) = fixture();
    let seeded = PersonaPreference {
        bank_id: "b-rec".into(),
        key: "brevity".into(),
        value: 0.4,
        trend: 0.1,
        confidence: None,
        source: "inferred".into(),
        source_ref: None,
        context: Some("c1".into()),
        admission_score: 0.9,
        as_of: "2026-01-02".into(),
    };
    store
        .put_preference_state(&seeded, "seed")
        .expect("seed pref");
    // Equal as_of, identical fields, context omitted: `pref_equal` treats the
    // absent wire context as no-difference, so the branch self-heals.
    let m = mut_for(
        "m-ppdup",
        "persona",
        "brevity",
        "2026-01-02T00:00:00Z",
        json!({
            "record": "preference",
            "entry": {"key": "brevity", "value": 0.4, "trend": 0.1,
                      "source": "inferred", "admission_score": 0.9,
                      "as_of": "2026-01-02"},
        }),
    );
    assert_eq!(apply(&store, None, &m), ApplyOutcome::Applied);
    assert!(store.journal_get("b-rec", "m-ppdup").unwrap().is_some());
    assert_eq!(store.conflict_count("b-rec").unwrap(), 0);
}

/// Item (5): a context-only belief correction rides a `LocalAhead`
/// merge, now returns Applied and is journaled, so it can be relayed to a
/// third peer via pull.
#[test]
fn belief_context_only_correction_applies_and_journals() {
    let (store, _kms) = fixture();
    let mut seed = belief("b-1", "s1", 1);
    seed.context = Some("kept".into());
    store.create_belief(&seed, "seed").expect("seed belief");
    let mut corrected = seed;
    corrected.context = Some("corrected".into());
    let m = mut_for(
        "m-bcorr",
        "belief",
        "b-1",
        "2026-01-02T00:00:00Z",
        serde_json::to_value(&corrected).expect("serialize"),
    );
    assert_eq!(apply(&store, None, &m), ApplyOutcome::Applied);
    let got = store.get_belief("b-rec", "b-1").unwrap().expect("belief");
    assert_eq!(got.context.as_deref(), Some("corrected"));
    assert!(store.journal_get("b-rec", "m-bcorr").unwrap().is_some());
    assert_eq!(store.conflict_count("b-rec").unwrap(), 0);
}

/// item (4): a diverged belief mutation loses with its stored context
/// UNCHANGED — the replicated correction only lands on an applying outcome.
#[test]
fn belief_diverged_mutation_leaves_context_unchanged() {
    let (store, _kms) = fixture();
    let mut seed = belief("b-1", "s1", 2);
    seed.context = Some("kept".into());
    store.create_belief(&seed, "seed").expect("seed belief");
    let mut diverged = seed;
    diverged.context = Some("corrected".into());
    diverged.confidence_history[1].confidence = 0.42;
    let m = mut_for(
        "m-bdiv",
        "belief",
        "b-1",
        "2026-01-03T00:00:00Z",
        serde_json::to_value(&diverged).expect("serialize"),
    );
    assert_eq!(apply(&store, None, &m), ApplyOutcome::ConflictLoser);
    let got = store.get_belief("b-rec", "b-1").unwrap().expect("belief");
    assert_eq!(got.context.as_deref(), Some("kept"));
    assert_eq!(got.confidence_history.len(), 2);
    assert!(
        (got.confidence_history[1].confidence - 0.7).abs() < 1e-12,
        "confidence = {}",
        got.confidence_history[1].confidence
    );
    let conflicts = store.conflict_list("b-rec", 10).unwrap();
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].loser_mutation_id, "m-bdiv");
}

/// item (2): a persona payload that carries a foreign `bank_id` field
/// fail-closes to the quarantine instead of being silently re-banked.
#[test]
fn persona_bank_mismatch_dead_letters() {
    let (store, _kms) = fixture();
    let mut foreign = stable_payload(&stable_entry("2026-01-02", true));
    foreign["entry"]["bank_id"] = json!("b-other");
    let m = mut_for("m-pfb", "persona", "style", "2026-01-01T00:00:00Z", foreign);
    assert_eq!(
        apply(&store, None, &m),
        ApplyOutcome::DeadLettered {
            reason_code: "unauthorized_bank".into()
        }
    );
    let foreign_pref = json!({
        "record": "preference",
        "entry": {"key": "brevity", "value": 0.4, "trend": 0.1,
                  "source": "inferred", "bank_id": "b-other",
                  "admission_score": 0.9, "as_of": "2026-01-02"},
    });
    let mp = mut_for(
        "m-ppfb",
        "persona",
        "brevity",
        "2026-01-02T00:00:00Z",
        foreign_pref,
    );
    assert_eq!(
        apply(&store, None, &mp),
        ApplyOutcome::DeadLettered {
            reason_code: "unauthorized_bank".into()
        }
    );
    let doc = store.get_persona_document("b-rec").expect("doc");
    assert!(doc.stable.is_empty(), "{:?}", doc.stable);
}

/// Sanity: a persona payload whose injected `bank_id` equals the envelope
/// bank still applies (the check is equality, not presence).
#[test]
fn persona_matching_wire_bank_id_still_applies() {
    let (store, _kms) = fixture();
    let mut ok = stable_payload(&stable_entry("2026-01-02", true));
    ok["entry"]["bank_id"] = json!("b-rec");
    let m = mut_for("m-pok", "persona", "style", "2026-01-01T00:00:00Z", ok);
    assert_eq!(apply(&store, None, &m), ApplyOutcome::Applied);
}

fn belief(id: &str, subject: &str, entries: usize) -> BeliefObject {
    let history: Vec<_> = (1..=entries)
        .map(|n| {
            ConfidenceEntry::new(
                format!("2026-01-0{n}"),
                0.5 + 0.1 * f64::from(u32::try_from(n).unwrap()),
                format!("ev-{n}"),
                SourceType::UserStated,
            )
            .expect("entry")
        })
        .collect();
    BeliefObject {
        id: id.into(),
        bank_id: "b-rec".into(),
        subject_id: subject.into(),
        proposition: "user prefers terse replies".into(),
        context: None,
        context_as_of: None,
        confidence_history: history,
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-01-02T00:00:00Z".into(),
    }
}
