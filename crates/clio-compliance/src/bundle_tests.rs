// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the shared bundle contract:
//! content-mode parsing, canonical checksum stability, envelope validation,
//! and round-trip entity deserialization.
//!
//! ## Owns
//! - Unit checks for `bundle.rs` pure helpers
//!
//! ## Does not own
//! - Store-backed export/import flows (see `export_tests` / `import_tests`)
//!
//! ## Boundary
//!
//! Changes to bundle helper behavior should normally require changes here.
//! Changes to export/import orchestration should not require changes here.
//!
//! Keep this module focused on bundle contract checks.

use super::*;
use clio_store::ComplianceTombstoneView;
use clio_types::{EpistemicKind, ErrorCode, ItemKind, MemoryItem, SemanticCategory};
use serde_json::Value;

fn sample_item() -> MemoryItem {
    MemoryItem {
        id: "itm-1".into(),
        bank_id: "bank-a".into(),
        subject_id: "subj-1".into(),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::Schema),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.9),
        admission_score: Some(0.75),
        source_ref: Some("turn_1".into()),
        snapshot: Some(serde_json::json!({"port": 5432})),
        gist: Some("a schema fact".into()),
        valid_from: "2026-01-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2026-01-01T00:00:00Z".into(),
        tx_until: None,
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-01-01T00:00:00Z".into(),
        context: None,
    }
}

fn sample_entities() -> ExportEntities {
    ExportEntities {
        items: vec![sample_item()],
        ..ExportEntities::default()
    }
}

#[test]
fn t22_bundle_checksum_is_deterministic() {
    let mut a = sample_entities();
    let mut b = sample_entities();
    // Entity order normalization happens in the exporter (sort_stable);
    // the checksum is stable for identical payloads.
    b.items.reverse();
    a.items.reverse();
    assert_eq!(entities_checksum(&a), entities_checksum(&b));
}

#[test]
fn t22_bundle_roundtrip_through_value() {
    let entities = sample_entities();
    let bytes = canonical_entities_bytes(&entities);
    let parsed: Value = serde_json::from_slice(&bytes).unwrap();
    let re: ExportEntities = serde_json::from_value(parsed).unwrap();
    assert_eq!(entities_checksum(&entities), entities_checksum(&re));
}

#[test]
fn t22_bundle_content_mode_parse_defaults_to_dsar() {
    assert_eq!(
        ExportContentMode::parse(None).unwrap(),
        ExportContentMode::DsarPlaintext
    );
    assert!(ExportContentMode::parse(Some("yaml")).is_err());
}

#[test]
fn t22_bundle_envelope_roundtrip_and_version_gate() {
    let manifest = CompletenessManifest {
        format_version: BUNDLE_FORMAT_VERSION,
        content_mode: ExportContentMode::DsarPlaintext,
        bank_ids: vec!["bank-a".into()],
        counts_by_type: sample_entities().counts_by_type(),
        counts_by_category: vec![],
        content_sha256: entities_checksum(&sample_entities()),
        filter: serde_json::json!({}),
        generated_at: "2026-09-20T00:00:00Z".into(),
        complete: true,
        omissions: vec![],
    };
    let envelope = BundleEnvelope {
        format: BUNDLE_FORMAT.into(),
        format_version: BUNDLE_FORMAT_VERSION,
        content_mode: ExportContentMode::DsarPlaintext,
        manifest,
        entities: sample_entities(),
    };
    let raw = serde_json::to_value(&envelope).unwrap();
    let parsed = parse_bundle(&raw).unwrap();
    assert_eq!(parsed.envelope, envelope);
    assert!(validate_envelope(&parsed.envelope).is_ok());
}

#[test]
fn t22_bundle_rejects_foreign_format_and_future_version() {
    let mut env = serde_json::json!({
        "format": "other.format",
        "format_version": 1,
        "content_mode": "dsar_plaintext",
        "manifest": {},
        "entities": {}
    });
    assert!(parse_bundle(&env).is_err());
    env["format"] = serde_json::json!(BUNDLE_FORMAT);
    env["format_version"] = serde_json::json!(BUNDLE_FORMAT_VERSION + 1);
    assert!(parse_bundle(&env).is_err());
}

#[test]
fn t22_bundle_content_mode_wire_strings_and_ciphertext_parse() {
    assert_eq!(ExportContentMode::DsarPlaintext.as_str(), "dsar_plaintext");
    assert_eq!(
        ExportContentMode::CiphertextBackup.as_str(),
        "ciphertext_backup"
    );
    assert_eq!(
        ExportContentMode::parse(Some("ciphertext_backup")).unwrap(),
        ExportContentMode::CiphertextBackup
    );
}

fn tombstone(request_id: &str, item_hash: Option<&str>) -> ComplianceTombstoneView {
    ComplianceTombstoneView {
        request_id: request_id.into(),
        legal_basis: "GDPR_ART_17".into(),
        subject_hash: format!("subject-{request_id}"),
        item_id_hash: item_hash.map(str::to_owned),
        category: Some("schema".into()),
        reason_code: "COMPLIANCE_CRYPTO_SHRED".into(),
        deleted_at: "2026-06-01T00:00:00Z".into(),
    }
}

#[test]
fn bundle_tombstones_canonicalize_deterministically() {
    let rows = vec![
        tombstone("req-b", None),
        tombstone("req-a", Some("hash-z")),
        tombstone("req-a", Some("hash-a")),
    ];
    let a = ExportEntities {
        tombstones: rows.clone(),
        ..ExportEntities::default()
    };
    let b = ExportEntities {
        tombstones: rows.into_iter().rev().collect(),
        ..ExportEntities::default()
    };
    assert_eq!(entities_checksum(&a), entities_checksum(&b));

    let bytes = canonical_entities_bytes(&a);
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    let hashes: Vec<Option<String>> = value["tombstones"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["item_id_hash"].as_str().map(str::to_owned))
        .collect();
    assert_eq!(
        hashes,
        vec![Some("hash-a".into()), Some("hash-z".into()), None],
        "non-null hashes sort first (NULLS LAST), null sorts last"
    );
}

#[test]
fn bundle_tombstones_with_tied_identity_use_total_order() {
    let mut first = tombstone("req-a", Some("hash-a"));
    first.deleted_at = "2026-06-01T00:00:00Z".into();
    let mut second = first.clone();
    second.deleted_at = "2026-06-02T00:00:00Z".into();
    let forward = ExportEntities {
        tombstones: vec![first.clone(), second.clone()],
        ..ExportEntities::default()
    };
    let backward = ExportEntities {
        tombstones: vec![second, first],
        ..ExportEntities::default()
    };
    assert_eq!(entities_checksum(&forward), entities_checksum(&backward));
}

#[test]
fn bundle_canonical_value_ignores_non_object_and_non_array() {
    let value = serde_json::json!({"items": 5, "triples": []});
    let bytes = canonical_value_bytes(&value);
    let re: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(re, value);
    assert_eq!(canonical_value_bytes(&serde_json::json!(7)), b"7");
}

#[test]
fn bundle_rejects_zero_format_version() {
    let entities = sample_entities();
    let mut envelope = BundleEnvelope {
        format: BUNDLE_FORMAT.into(),
        format_version: BUNDLE_FORMAT_VERSION,
        content_mode: ExportContentMode::DsarPlaintext,
        manifest: CompletenessManifest {
            format_version: BUNDLE_FORMAT_VERSION,
            content_mode: ExportContentMode::DsarPlaintext,
            bank_ids: vec!["bank-a".into()],
            counts_by_type: entities.counts_by_type(),
            counts_by_category: vec![],
            content_sha256: entities_checksum(&entities),
            filter: serde_json::json!({}),
            generated_at: "2026-09-20T00:00:00Z".into(),
            complete: true,
            omissions: vec![],
        },
        entities,
    };
    assert!(validate_envelope(&envelope).is_ok());
    envelope.format_version = 0;
    let err = validate_envelope(&envelope).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
}
