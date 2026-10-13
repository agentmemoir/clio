// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the domain-record sync apply effects: persona stable
//! entry / preference applies with context preservation, belief create and
//! append-only merges under the context rule, erasure fail-closed
//! checks, and the explicit not-implemented dead-lettering of task/failure
//! records.
//!
//! ## Owns
//! - Behavior coverage for `apply_records`
//! - The rule that a replicated write leaves audited rows with no
//!   presenting-credential key
//!
//! ## Does not own
//! - Item/assoc/triple apply tests (`apply_tests` / `apply_matrix_tests`)
//! - Push-side packaging tests (`client_feed_tests`)
//!
//! ## Boundary
//!
//! Tests here break when record apply semantics change; they never change it.
//!
//! Keep this module focused on domain-record apply tests.

use crate::apply::ApplyOutcome;
use crate::apply::apply_tests::{apply, env, fixture, item, item_value};
use crate::protocol::{SyncContent, SyncMutation};
use clio_store::{BeliefStore, PersonaStore, Store};
use clio_types::{ConfidenceEntry, SourceType};
use serde_json::{Value, json};

use clio_types::{BeliefObject, PersonaPreference, PersonaStableEntry};

fn stable(key: &str, value: &str, as_of: &str, score: f64) -> PersonaStableEntry {
    PersonaStableEntry {
        bank_id: "b-rec".into(),
        key: key.into(),
        value: value.into(),
        confidence: None,
        source: "explicit".into(),
        source_ref: Some("ev".into()),
        context: None,
        admission_score: score,
        as_of: as_of.into(),
        valid_until: None,
    }
}

fn payload(record: &str, entry: &impl serde::Serialize) -> Value {
    json!({ "record": record,
            "entry": serde_json::to_value(entry).expect("serialize entry") })
}

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

/// Regression: a legacy peer payload without a `context` key must
/// preserve the stored context on an item update (no silent clear).
/// The persona erase regression is not missing: it lives in
/// `apply_records_edge_tests::persona_preference_erased_bank_dead_letters`,
/// which drives the bank-level `persona_context_erased` tombstone probe that
/// item 5 put in place of the old `subject_is_shredded(&m.bank)` check.
#[test]
fn f14_legacy_item_payload_preserves_stored_context_on_update() {
    let (store, kms) = fixture();
    let bank = "b-ctx-f14";
    let mut seed = item(bank, "i-ctx", "s1", "2026-01-01T00:00:00Z", "v1");
    seed.context = Some("why recorded".into());
    store.create_memory_item(&seed, "seed").expect("seed item");
    let payload = item_value(bank, "i-ctx", "s1", "2026-01-02T00:00:00Z", "v2");
    assert!(payload.get("context").is_none());
    let m = env(
        "m-f14",
        bank,
        "item",
        "i-ctx",
        "upsert",
        "2026-01-02T00:00:00Z",
        "dev-b",
        payload,
    );
    assert_eq!(apply(&store, Some(kms.as_ref()), &m), ApplyOutcome::Applied);
    let got = store.get_memory_item(bank, "i-ctx").unwrap().expect("item");
    assert_eq!(got.gist.as_deref(), Some("v2"));
    assert_eq!(got.context.as_deref(), Some("why recorded"));
}

/// persona stable entries apply end to end; re-delivery dedupes.
#[test]
fn persona_stable_mutation_applies_then_duplicates() {
    let (store, _kms) = fixture();
    let m = mut_for(
        "m-p1",
        "persona",
        "style",
        "2026-01-01T00:00:00Z",
        payload("stable", &stable("style", "terse", "2026-01-01", 0.5)),
    );
    assert_eq!(apply(&store, None, &m), ApplyOutcome::Applied);
    assert_eq!(apply(&store, None, &m), ApplyOutcome::Duplicate);
    let doc = store.get_persona_document("b-rec").expect("persona doc");
    assert_eq!(doc.stable.len(), 1);
    assert_eq!(doc.stable[0].value, "terse");
}

#[test]
fn persona_stable_older_version_conflicts_and_absent_context_preserves() {
    let (store, _kms) = fixture();
    let first = mut_for("m-p1", "persona", "style", "2026-01-02T00:00:00Z", {
        let mut e = stable("style", "terse", "2026-01-02", 0.5);
        e.context = Some("why recorded".into());
        payload("stable", &e)
    });
    assert_eq!(apply(&store, None, &first), ApplyOutcome::Applied);
    // An older as_of loses deterministically.
    let older = mut_for(
        "m-p0",
        "persona",
        "style",
        "2026-01-01T00:00:00Z",
        payload("stable", &stable("style", "brief", "2026-01-01", 0.5)),
    );
    assert_eq!(apply(&store, None, &older), ApplyOutcome::ConflictLoser);
    // A newer version without a `context` key preserves the stored one.
    let newer = mut_for(
        "m-p2",
        "persona",
        "style",
        "2026-01-03T00:00:00Z",
        payload("stable", &stable("style", "compact", "2026-01-03", 0.5)),
    );
    assert_eq!(apply(&store, None, &newer), ApplyOutcome::Applied);
    let doc = store.get_persona_document("b-rec").expect("doc");
    assert_eq!(doc.stable[0].value, "compact");
    assert_eq!(doc.stable[0].context.as_deref(), Some("why recorded"));
}

/// A replicated record is applied on the receiving peer, whose identity is a
/// transport-level fact and not part of the payload. The audited rows the apply
/// leaves therefore carry no presenting-credential key: an origin actor is data,
/// and the peer's own credential belongs to the transport.
#[test]
fn a_replicated_write_leaves_audit_rows_with_no_credential_key() {
    let (store, _kms) = fixture();
    let m = mut_for(
        "m-nopr",
        "persona",
        "style",
        "2026-01-01T00:00:00Z",
        payload("stable", &stable("style", "terse", "2026-01-01", 0.5)),
    );
    assert_eq!(apply(&store, None, &m), ApplyOutcome::Applied);
    let trail = store
        .audit_trail("b-rec", "persona-stable:style")
        .expect("persona audit trail");
    assert!(
        !trail.events.is_empty(),
        "the apply left an audited persona write"
    );
    for event in &trail.events {
        assert!(
            event.detail.get("principal").is_none(),
            "a replicated row must not gain a credential key: {}",
            event.detail
        );
    }
}

/// Exact restored values from the payload (bit-identical round trip).
#[allow(clippy::float_cmp)]
#[test]
fn persona_preference_applies_and_preserves_context_when_absent() {
    let (store, _kms) = fixture();
    let mut seed = PersonaPreference {
        bank_id: "b-rec".into(),
        key: "brevity".into(),
        value: 0.4,
        trend: 0.1,
        confidence: None,
        source: "inferred".into(),
        source_ref: None,
        context: Some("c1".into()),
        admission_score: 0.5,
        as_of: "2026-01-01".into(),
    };
    seed.bank_id = "b-rec".into();
    store
        .put_preference_state(&seed, "seed")
        .expect("seed pref");
    let mut incoming = seed;
    incoming.value = 0.6;
    incoming.as_of = "2026-01-02".into();
    incoming.context = None;
    let m = mut_for(
        "m-pp",
        "persona",
        "brevity",
        "2026-01-02T00:00:00Z",
        payload("preference", &incoming),
    );
    assert_eq!(apply(&store, None, &m), ApplyOutcome::Applied);
    let got = store
        .get_preference("b-rec", "brevity")
        .unwrap()
        .expect("pref");
    assert_eq!(got.value, 0.6);
    assert_eq!(got.context.as_deref(), Some("c1"));
}

#[test]
fn belief_creates_appends_and_redelivery_duplicates() {
    let (store, _kms) = fixture();
    let m1 = mut_for(
        "m-b1",
        "belief",
        "b-1",
        "2026-01-01T00:00:00Z",
        serde_json::to_value(belief("b-1", "s1", 1)).expect("serialize"),
    );
    assert_eq!(apply(&store, None, &m1), ApplyOutcome::Applied);
    // Same proposition, one more entry: the missing tail is appended.
    let mut grown = belief("b-1", "s1", 2);
    grown.bank_id = "b-rec".into();
    let m2 = mut_for(
        "m-b2",
        "belief",
        "b-1",
        "2026-01-02T00:00:00Z",
        serde_json::to_value(&grown).expect("serialize"),
    );
    assert_eq!(apply(&store, None, &m2), ApplyOutcome::Applied);
    let got = store.get_belief("b-rec", "b-1").unwrap().expect("belief");
    assert_eq!(got.confidence_history.len(), 2);
    // Re-delivery of either mutation is a documented no-op.
    assert_eq!(apply(&store, None, &m1), ApplyOutcome::Duplicate);
    assert_eq!(apply(&store, None, &m2), ApplyOutcome::Duplicate);
    // A trajectory the store already supersedes adds nothing.
    let stale = mut_for(
        "m-b3",
        "belief",
        "b-1",
        "2026-01-01T00:00:00Z",
        serde_json::to_value(belief("b-1", "s1", 1)).expect("serialize"),
    );
    assert_eq!(apply(&store, None, &stale), ApplyOutcome::Duplicate);
}

/// (5): a context-only belief correction (a `LocalAhead` merge with a
/// differing wire context) returns Applied and is journaled, so it can be
/// relayed to a third peer — see `apply_records_selfheal_tests`. Persona apply
/// probes the bank-level `persona erase` tombstone
/// (`persona_context_erased`), and that regression is covered in
/// `apply_records_edge_tests::persona_preference_erased_bank_dead_letters`.
#[test]
fn belief_context_rule_omitted_preserves_and_differing_corrects() {
    let (store, _kms) = fixture();
    let mut seed = belief("b-1", "s1", 1);
    seed.context = Some("kept".into());
    store.create_belief(&seed, "seed").expect("seed belief");
    // Assertion updated to the corrected contract: a differing context is a
    // replicated authorized correction (appends are gated locally, so the
    // sender can only have reached it through `correct_belief_context`), and it
    // applies through the audited correction path instead of dead-lettering.
    let mut differing = seed;
    differing.context = Some("other".into());
    differing
        .confidence_history
        .push(ConfidenceEntry::new("2026-01-03", 0.9, "ev-3", SourceType::UserStated).unwrap());
    let corrected = mut_for(
        "m-bx",
        "belief",
        "b-1",
        "2026-01-03T00:00:00Z",
        serde_json::to_value(&differing).expect("serialize"),
    );
    assert_eq!(apply(&store, None, &corrected), ApplyOutcome::Applied);
    let got = store.get_belief("b-rec", "b-1").unwrap().expect("belief");
    assert_eq!(got.context.as_deref(), Some("other"));
    // Absent context appends and preserves the stored value: the
    // incoming trajectory extends the stored one (context omitted on the wire).
    let mut quiet = got;
    quiet.context = None;
    quiet
        .confidence_history
        .push(ConfidenceEntry::new("2026-01-04", 0.8, "ev-4", SourceType::UserStated).unwrap());
    let ok = mut_for(
        "m-b2",
        "belief",
        "b-1",
        "2026-01-04T00:00:00Z",
        serde_json::to_value(&quiet).expect("serialize"),
    );
    assert_eq!(apply(&store, None, &ok), ApplyOutcome::Applied);
    let got = store.get_belief("b-rec", "b-1").unwrap().expect("belief");
    assert_eq!(got.context.as_deref(), Some("other"));
    assert_eq!(got.confidence_history.len(), 3);
}

#[test]
fn belief_shredded_subject_dead_letters() {
    let (store, _kms) = fixture();
    store
        .upsert_subject_key_ref("s-gone", "shredded", "shredded")
        .expect("shred marker");
    let m = mut_for(
        "m-bs",
        "belief",
        "b-1",
        "2026-01-01T00:00:00Z",
        serde_json::to_value(belief("b-1", "s-gone", 1)).expect("serialize"),
    );
    assert_eq!(
        apply(&store, None, &m),
        ApplyOutcome::DeadLettered {
            reason_code: "erased_subject".into()
        }
    );
}

/// Task/failure records now have structured apply effects, so this test
/// no longer pins the old "`unsupported_op`" dead-letter contract. A task/failure
/// payload that is present but malformed (missing carrier item / record) is
/// quarantined as `malformed`, exactly like a malformed persona payload. The
/// positive apply path is covered below and end to end in `client_feed_tests`.
#[test]
fn task_and_failure_malformed_payloads_dead_letter_and_malformed_persona_rejects() {
    let (store, _kms) = fixture();
    let task = mut_for("m-t", "task", "t-1", "2026-01-01T00:00:00Z", json!({}));
    assert_eq!(
        apply(&store, None, &task),
        ApplyOutcome::DeadLettered {
            reason_code: "malformed".into()
        },
        "a task payload missing its carrier/record is quarantined"
    );
    let failure = mut_for("m-f", "failure", "f-1", "2026-01-01T00:00:00Z", json!({}));
    assert_eq!(
        apply(&store, None, &failure),
        ApplyOutcome::DeadLettered {
            reason_code: "malformed".into()
        }
    );
    let bad = mut_for(
        "m-pb",
        "persona",
        "style",
        "2026-01-01T00:00:00Z",
        json!({"record": "other"}),
    );
    assert_eq!(
        apply(&store, None, &bad),
        ApplyOutcome::DeadLettered {
            reason_code: "malformed".into()
        }
    );
}

#[cfg(test)]
#[path = "apply_records_tests_edge_tests.rs"]
mod apply_records_edge_tests;
