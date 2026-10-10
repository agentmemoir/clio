// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing handle resolution: that each handled
//! kind gets its real identifier back out of its own decrypted payload, that
//! kinds which never had a handle pass through untouched, and that a payload
//! missing its identifier is refused with a reason that names the kind and the
//! bank and leaks nothing else.
//!
//! ## Owns
//! - The tests for `resolve_entity_id` and `ResolveFailure::reason`
//!
//! ## Does not own
//! - The handle derivation and the handled kind set (`keyed_roles_tests`)
//! - Applying a resolved mutation (`apply`'s tests)
//! - The relay's stored journal bytes (`client`'s wire tests)
//!
//! ## Boundary
//!
//! Changes to where a handled kind's identifier lives in its payload should
//! normally require changes here. Changes to apply semantics should not.
//!
//! Keep this module focused on handle-resolution tests.

use super::{ResolveFailure, resolve_entity_id};
use crate::protocol::{
    ENTITY_KIND_ASSOC, ENTITY_KIND_BELIEF, ENTITY_KIND_FAILURE, ENTITY_KIND_ITEM,
    ENTITY_KIND_PERSONA, ENTITY_KIND_TASK, SyncContent, SyncMutation,
};
use serde_json::json;

/// A mutation as it arrives over the wire: an opaque identifier in the
/// identifier field, the payload still sealed.
fn wire(kind: &str, bank: &str, entity_id: &str) -> SyncMutation {
    SyncMutation {
        sync_mutation_id: "mut-1".into(),
        bank: bank.into(),
        entity_kind: kind.into(),
        entity_id: entity_id.into(),
        op: "upsert".into(),
        transaction_time: "2026-01-01T00:00:00Z".into(),
        device_id: "dev-1".into(),
        content: SyncContent::Cipher(crate::crypto::SyncCipher {
            alg: "xchacha20poly1305".into(),
            key_id: None,
            nonce_b64: String::new(),
            ciphertext_b64: String::new(),
        }),
        content_hash: "digest".into(),
        admission_receipt: None,
    }
}

/// Each handled kind's real identifier is read back out of the field its own
/// payload carries, which is what lets the apply matrix's identifier checks pass
/// against a resolved row.
#[test]
fn each_handled_kind_resolves_from_its_own_payload() {
    let cases: [(&str, serde_json::Value, &str); 4] = [
        (
            ENTITY_KIND_PERSONA,
            json!({"record": "stable", "entry": {"key": "communication_style"}}),
            "communication_style",
        ),
        (
            ENTITY_KIND_TASK,
            json!({"task": {"task_id": "task-42", "id": "task-row-1"}}),
            "task-42",
        ),
        (
            ENTITY_KIND_FAILURE,
            json!({"failure": {"id": "fr-b1-task-42-1-0"}}),
            "fr-b1-task-42-1-0",
        ),
        (
            ENTITY_KIND_ASSOC,
            json!({"id": "expl\u{1f}b1\u{1f}i-1\u{1f}i-2\u{1f}mentor"}),
            "expl\u{1f}b1\u{1f}i-1\u{1f}i-2\u{1f}mentor",
        ),
    ];
    for (kind, payload, want) in cases {
        let mut m = wire(kind, "b1", "3f2a9c8b7d6e5f40");
        resolve_entity_id(&mut m, &payload).expect("a payload carrying its identifier resolves");
        assert_eq!(m.entity_id, want, "{kind} resolves its real identifier");
    }
}

/// A machine-generated identifier was never turned into a handle, so a
/// resolved row for those kinds is exactly the row that arrived. A `belief`
/// row in particular keeps its identifier, which is what the shared packaging
/// helper's non-persona arm must not disturb.
#[test]
fn an_unhandled_kind_passes_through_untouched() {
    for kind in [ENTITY_KIND_ITEM, ENTITY_KIND_BELIEF] {
        let mut m = wire(kind, "b1", "itm-1700000000-1-0-42");
        let before = m.entity_id.clone();
        resolve_entity_id(&mut m, &json!({"id": before})).expect("an unhandled kind never fails");
        assert_eq!(
            m.entity_id, before,
            "{kind} keeps the identifier it arrived with"
        );
    }
}

/// A handled row whose payload carries no matching identifier is refused. The
/// mutation is left exactly as it arrived, so nothing downstream can apply it
/// with the handle still standing in for an identifier.
#[test]
fn a_payload_without_its_identifier_is_refused() {
    let mut m = wire(ENTITY_KIND_TASK, "b1", "3f2a9c8b7d6e5f40");
    let before = m.entity_id.clone();
    let outcome = resolve_entity_id(&mut m, &json!({"item": {"id": "i-1"}}));
    assert!(
        matches!(outcome, Err(ResolveFailure::MissingIdentifier)),
        "a task payload with no task record must not resolve"
    );
    assert_eq!(
        m.entity_id, before,
        "the refused mutation is untouched, never partially resolved"
    );
}

/// The refusal names the kind and the bank so the operator can find the row,
/// and names neither the wire handle nor the missing identifier.
#[test]
fn the_refusal_names_the_kind_and_bank_and_nothing_else() {
    let reason = ResolveFailure::MissingIdentifier.reason(ENTITY_KIND_TASK, "bank-7");
    assert!(reason.contains("task"), "names the kind: {reason}");
    assert!(reason.contains("bank-7"), "names the bank: {reason}");
    assert!(
        !reason.contains("3f2a9c8b"),
        "never echoes the wire handle: {reason}"
    );
    assert!(
        !reason.to_lowercase().contains("task-42"),
        "never echoes a real identifier: {reason}"
    );
}

/// A persona preference carries its identifier under the same field as a
/// stable entry, so one resolution path covers both payload shapes.
#[test]
fn a_persona_preference_resolves_through_the_same_field() {
    let mut m = wire(ENTITY_KIND_PERSONA, "b1", "3f2a9c8b7d6e5f40");
    let payload = json!({"record": "preference", "entry": {"key": "brevity_affinity"}});
    resolve_entity_id(&mut m, &payload).expect("the preference shape resolves");
    assert_eq!(m.entity_id, "brevity_affinity");
}
