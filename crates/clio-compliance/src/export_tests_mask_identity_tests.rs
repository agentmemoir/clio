// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for export-side masking identity tests:
//! the per-record `ciphertext_backup` placeholders for identity-bearing prose
//! (belief propositions, triple SPO fields, assoc-edge relationships), the
//! plaintext triple/assoc-edge scrub, and the constant opaque prose for
//! non-identity fields.
//!
//! ## Owns
//! - Export-side masking assertions over real in-memory SQLite stores
//!
//! ## Does not own
//! - The restore round trip (`backup_restore_tests.rs`)
//! - Shared export fixtures (`export_tests.rs`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::bundle::{BundleEnvelope, ExportContentMode};
use crate::export::{ExportFilter, export};
use crate::export_mask::OPAQUE_CONTENT;
use crate::export_tests::{T, args, belief, ctx, fixture, item, read, triple};
use crate::test_unique::unique;
use clio_store::{AssocEdgeRecord, AssocKind, BeliefStore, Store};
use clio_types::{Actor, EpisodicType};

const SECRET: &str = "api_key: sk-test-1234567890";
const SECRET_MASKED: &str = "api_key: ****7890";

/// Adds one belief with a distinct id/proposition and one triple with a
/// distinct id/object (both with carrier items) to the shared fixture.
fn seed_second_belief_and_triple(f: &crate::export_tests::Fixture) {
    let mut belief_row = belief(&f.bank);
    belief_row.id = "belief-2".into();
    belief_row.proposition = "user prefers rye bread".into();
    f.store.create_belief(&belief_row, "tester").unwrap();

    let carrier = item(
        "tri-2",
        &f.bank,
        "sub-c",
        None,
        Some(EpisodicType::Triple),
        None,
    );
    let mut triple_row = triple(&f.bank);
    triple_row.id = "triple-2".into();
    triple_row.item_id = carrier.id.clone();
    triple_row.predicate = "prefers".into();
    triple_row.object = "oat milk".into();
    f.store
        .commit_triple_add(&carrier, &triple_row, false, "tester")
        .unwrap();
}

/// In `ciphertext_backup` mode the identity-bearing prose of every
/// record gets a deterministic content-free per-record placeholder, so records
/// stay distinct after masking; the non-identity prose stays the constant.
#[test]
fn backup_placeholders_stay_distinct_per_record() {
    let f = fixture("mask-backup-identity");
    seed_second_belief_and_triple(&f);
    let ctx = ctx(&f, Actor::Harness);
    export(&args(
        &f,
        &ctx,
        ExportFilter::default(),
        ExportContentMode::CiphertextBackup,
    ))
    .expect("export succeeds");
    let envelope: BundleEnvelope = serde_json::from_str(&read(&f)).unwrap();
    let e = &envelope.entities;

    // Belief propositions: one placeholder per belief id.
    let mut propositions: Vec<&str> = e.beliefs.iter().map(|b| b.proposition.as_str()).collect();
    propositions.sort_unstable();
    assert_eq!(
        propositions,
        vec![
            "[ciphertext_backup: content sealed belief-1]",
            "[ciphertext_backup: content sealed belief-2]",
        ]
    );

    // Triple SPO fields: per-record AND per-field placeholders.
    for t in &e.triples {
        assert_eq!(
            t.subject,
            format!("[ciphertext_backup: content sealed {}:subject]", t.id)
        );
        assert_eq!(
            t.predicate,
            format!("[ciphertext_backup: content sealed {}:predicate]", t.id)
        );
        assert_eq!(
            t.object,
            format!("[ciphertext_backup: content sealed {}:object]", t.id)
        );
    }
    assert!(e.triples.iter().any(|t| t.id == "triple-1"));
    assert!(e.triples.iter().any(|t| t.id == "triple-2"));

    // Non-identity prose stays the shared constant.
    assert!(e.tasks.iter().all(|t| t.definition == OPAQUE_CONTENT));
    assert!(
        e.failures
            .iter()
            .all(|r| r.what_failed == OPAQUE_CONTENT && r.lesson == OPAQUE_CONTENT)
    );
    assert!(e.persona_stable.iter().all(|r| r.value == OPAQUE_CONTENT));
}

/// `dsar_plaintext` scrubs triple SPO prose (non-secret text passes
/// through unchanged) — before the fix the fields were exported unscrubbed.
#[test]
fn plaintext_export_scrubs_secret_shaped_triple_fields() {
    let f = fixture("mask-plain-triple");
    let carrier = item(
        "tri-secret",
        &f.bank,
        "sub-c",
        None,
        Some(EpisodicType::Triple),
        None,
    );
    let mut secret_triple = triple(&f.bank);
    secret_triple.id = "triple-secret".into();
    secret_triple.item_id = carrier.id.clone();
    secret_triple.subject = "user".into();
    secret_triple.predicate = "stores".into();
    secret_triple.object = format!("uses {SECRET}");
    f.store
        .commit_triple_add(&carrier, &secret_triple, false, "tester")
        .unwrap();

    let ctx = ctx(&f, Actor::Harness);
    export(&args(
        &f,
        &ctx,
        ExportFilter::default(),
        ExportContentMode::DsarPlaintext,
    ))
    .expect("export succeeds");
    let text = read(&f);
    assert!(
        !text.contains("sk-test-1234567890"),
        "a raw secret in a triple field must not reach the bundle"
    );
    let envelope: BundleEnvelope = serde_json::from_str(&text).unwrap();
    let secret = envelope
        .entities
        .triples
        .iter()
        .find(|t| t.id == "triple-secret")
        .expect("secret triple exported");
    assert_eq!(secret.object, format!("uses {SECRET_MASKED}"));
    // Non-secret prose passes through unchanged.
    let plain = envelope
        .entities
        .triples
        .iter()
        .find(|t| t.id == "triple-1")
        .expect("plain triple exported");
    assert_eq!(
        (
            plain.subject.as_str(),
            plain.predicate.as_str(),
            plain.object.as_str()
        ),
        ("user", "likes", "rust")
    );
}

/// An assoc edge's free-text `relationship` is scrubbed in
/// `dsar_plaintext` mode and gets a per-record placeholder in
/// `ciphertext_backup` mode; non-text fields stay untouched.
#[test]
fn assoc_edge_relationship_is_masked_per_content_mode() {
    let f = fixture("mask-assoc");
    f.store
        .put_assoc_edge(&AssocEdgeRecord {
            id: "edge-secret".into(),
            bank_id: f.bank.clone(),
            source_id: "sem-1".into(),
            target_id: "epi-1".into(),
            assoc_kind: AssocKind::Explicit,
            relationship: Some(SECRET.into()),
            weight: 0.5,
            last_updated_at: T.into(),
        })
        .unwrap();

    for (mode, expected) in [
        (ExportContentMode::DsarPlaintext, SECRET_MASKED),
        (
            ExportContentMode::CiphertextBackup,
            "[ciphertext_backup: content sealed edge-secret:relationship]",
        ),
    ] {
        let ctx = ctx(&f, Actor::Harness);
        export(&args(&f, &ctx, ExportFilter::default(), mode)).expect("export succeeds");
        let envelope: BundleEnvelope = serde_json::from_str(&read(&f)).unwrap();
        let edge = envelope
            .entities
            .assoc_edges
            .iter()
            .find(|e| e.id == "edge-secret")
            .unwrap_or_else(|| panic!("{mode:?}: secret edge exported"));
        assert_eq!(
            edge.relationship.as_deref(),
            Some(expected),
            "{mode:?}: relationship masking"
        );
        // The numeric weight is metadata, not prose: it stays complete. `weight`
        // is an `f64` round-tripped bit-exactly through serde; compare with an
        // explicit tolerance instead of strict float equality.
        assert!(
            (edge.weight - 0.5).abs() < 1e-12,
            "weight = {}",
            edge.weight
        );
        // The id-less `None` edge from the shared seed is untouched.
        let none_edge = envelope
            .entities
            .assoc_edges
            .iter()
            .find(|e| e.id == "edge-1")
            .expect("seeded None-relationship edge exported");
        assert_eq!(none_edge.relationship, None);
    }
}

/// Guard: a per-record placeholder never collides with a real proposition —
/// the id suffix makes an accidental collision require an exact match on the
/// other record's id, and the sealed prefix marks it as masked content.
#[test]
fn placeholder_form_is_record_scoped() {
    let id = unique("bel");
    let proposition = format!("[ciphertext_backup: content sealed {id}]");
    // proposition_key lowercases but keeps the unique record id, so distinct
    // record ids stay distinct after masking.
    assert_eq!(
        clio_types::proposition_key(&proposition),
        proposition.to_lowercase()
    );
    assert!(proposition != OPAQUE_CONTENT);
}
