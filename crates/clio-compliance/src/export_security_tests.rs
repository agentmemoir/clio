// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the export security and content-mode
//! tests: secret masking, the fail-closed authorization gate, ciphertext
//! backup serialization, and degraded-content fail-closed paths.
//!
//! ## Owns
//! - Security / content-mode export assertions over the shared export fixtures
//!
//! ## Does not own
//! - Fixture builders (see `export_tests.rs`)
//! - Full-bank and filter tests (see `export_tests.rs`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::bundle::{BundleEnvelope, ExportContentMode};
use crate::export::{ExportArgs, ExportDelivery, export};
use crate::export_tests::{
    Fixture, Scratch, T, args, ctx, dest_path, export_at, fixture, item, read, staging_entries,
    store,
};
use crate::subject_request_events::BindingControls;
use crate::test_unique::unique;
use clio_store::{
    AssocEdgeRecord, AssocKind, ComplianceStore, DestinationCustodyStore, Store, SubjectEraseSpec,
};
use clio_types::{Actor, Bank, OpContext, SemanticCategory};

#[test]
fn t22_06_secrets_never_reach_the_file() {
    let f = fixture("mask");
    let ctx = ctx(&f, Actor::Harness);
    export(&args(
        &f,
        &ctx,
        crate::export::ExportFilter::default(),
        ExportContentMode::DsarPlaintext,
    ))
    .unwrap();
    let text = read(&f);
    assert!(!text.contains("sk-test-1234567890"));
    assert!(!text.contains("\"dek\""));
}

#[test]
fn t22_08_agent_actor_is_rejected_without_writing() {
    let f = fixture("denied");
    let ctx = ctx(&f, Actor::Agent);
    let err = export(&args(
        &f,
        &ctx,
        crate::export::ExportFilter::default(),
        ExportContentMode::DsarPlaintext,
    ))
    .unwrap_err();
    assert_eq!(err.code, clio_types::ErrorCode::Forbidden);
    assert!(!f.path.as_path().exists());
}

#[test]
fn empty_bank_is_rejected() {
    let f = fixture("empty-bank");
    let ctx = OpContext::new(Bank::new(String::new()), Actor::Harness);
    let err = export(&args(
        &f,
        &ctx,
        crate::export::ExportFilter::default(),
        ExportContentMode::DsarPlaintext,
    ))
    .unwrap_err();
    assert_eq!(err.code, clio_types::ErrorCode::MissingBank);
}

#[test]
fn unwritable_destination_fails() {
    let f = fixture("write-fail");
    let err = export_at(&f, "/nonexistent-am-export-dir/bundle.json").unwrap_err();
    assert_eq!(err.code, clio_types::ErrorCode::Internal);
}

#[test]
fn compliance_purged_content_fails_export_closed() {
    let store = store();
    let bank = unique("dbank");
    store
        .create_memory_item(
            &item(
                "gone-2",
                &bank,
                "sub-gone",
                Some(SemanticCategory::parse("persona").unwrap()),
                None,
                None,
            ),
            "tester",
        )
        .unwrap();
    store
        .create_memory_item(
            &item(
                "derived-1",
                &bank,
                "sub-b",
                Some(SemanticCategory::parse("schema").unwrap()),
                None,
                None,
            ),
            "tester",
        )
        .unwrap();
    store
        .put_assoc_edge(&AssocEdgeRecord {
            id: "hd-1".into(),
            bank_id: bank.clone(),
            source_id: "derived-1".into(),
            target_id: "gone-2".into(),
            assoc_kind: AssocKind::Explicit,
            relationship: Some("hub_distill".into()),
            weight: 0.5,
            last_updated_at: T.into(),
        })
        .unwrap();
    // A fresh salt per run: no test asserts the tombstone hash value.
    let salt = rand::random::<[u8; 16]>();
    store
        .erase_subject(&SubjectEraseSpec {
            bank_id: &bank,
            subject_id: "sub-gone",
            request_id: "req-2",
            legal_basis: "GDPR_ART_17",
            reason_code: "COMPLIANCE_CRYPTO_SHRED",
            deleted_at: T,
            salt: &salt,
        })
        .unwrap();
    let path = dest_path("derived");
    let f = Fixture { store, bank, path };
    let ctx = ctx(&f, Actor::Harness);
    let err = export(&args(
        &f,
        &ctx,
        crate::export::ExportFilter::default(),
        ExportContentMode::DsarPlaintext,
    ))
    .unwrap_err();
    // Erasure NULLs `content_ciphertext`, so the purged row reads back as
    // unreadable content rather than a backend fault. The export still fails
    // closed and writes nothing.
    assert_eq!(err.code, clio_types::ErrorCode::UnreadableContent);
    assert!(!f.path.as_path().exists());
}

#[test]
fn t22_09_ciphertext_backup_exports_opaque_items() {
    let f = fixture("cipher");
    let ctx = ctx(&f, Actor::Harness);
    let outcome = export(&args(
        &f,
        &ctx,
        crate::export::ExportFilter::default(),
        ExportContentMode::CiphertextBackup,
    ))
    .unwrap();
    let text = read(&f);
    assert!(!text.contains("\"snapshot\""));
    assert!(!text.contains("\"dek\""));
    let envelope: BundleEnvelope = serde_json::from_str(&text).unwrap();
    assert_eq!(envelope.entities.items.len(), 0);
    assert_eq!(envelope.entities.opaque_items.len(), 5);
    for opaque in &envelope.entities.opaque_items {
        assert_eq!(
            opaque.content_cipher,
            f.store
                .inspect_content_ciphertext(&f.bank, &opaque.id)
                .unwrap()
                .unwrap()
        );
        assert!(opaque.subject_id.starts_with("sub-"));
    }
    assert!(outcome.manifest.complete);
}

/// A directory destination must refuse the delivery before any write: the
/// export maps the refusal to `InvalidArgument`, the directory survives, the
/// staging directory stays empty, and the custody row settles as failed.
#[test]
fn a_directory_destination_refuses_the_export_with_invalid_argument() {
    let store = store();
    let scratch = Scratch::new();
    let dest = scratch.root.join("dest-dir");
    std::fs::create_dir_all(&dest).unwrap();
    let controls = BindingControls {
        copy_controls_accepted: true,
        ..BindingControls::default()
    };
    let ctx = OpContext::new(Bank::new(unique("bank")), Actor::Harness);
    let dest_str = dest.to_string_lossy().into_owned();
    let action_id = "act-dir";
    let staging = scratch.staging_dir();
    let export_args = ExportArgs {
        store: &store,
        compliance: &store,
        persona: &store,
        history: &store,
        belief: &store,
        ctx: &ctx,
        destination: &dest_str,
        filter: crate::export::ExportFilter::default(),
        content_mode: ExportContentMode::DsarPlaintext,
        timestamp: "2026-09-20T00:00:00Z",
        subject_id: None,
        facts: None,
        backend: None,
        delivery: Some(ExportDelivery {
            events: &store,
            custody: &store,
            controls: &controls,
            action_id,
            staging_dir: &staging,
            custody_owner: "clio",
            required_action: "delivery",
        }),
    };
    let err = export(&export_args).unwrap_err();
    assert_eq!(err.code, clio_types::ErrorCode::InvalidArgument);
    assert_eq!(err.message, "export destination is not a regular file");
    assert!(dest.is_dir());
    assert_eq!(staging_entries(&staging), [] as [std::path::PathBuf; 0]);
    let row = store.load(action_id).unwrap().expect("custody row");
    assert_eq!(row.state, "failed");
}
