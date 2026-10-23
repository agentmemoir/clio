// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the bundle import test suite: dry-run
//! safety, idempotent re-import, admission rejection, checksum-before-write,
//! and format/force validation.
//!
//! ## Owns
//! - Import behavior assertions over the shared `import_fixture` harness
//!
//! ## Does not own
//! - Fixture builders (see `import_fixture.rs`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use clio_admission::AdmissionPolicy;
use clio_config::ranking::RankingEnv;
use clio_store::{BeliefStore, HistoryStore, ItemInspectFilter, PersonaStore, Store};
use clio_types::{Actor, Bank, ErrorCode, OpContext};

use crate::bundle::{BUNDLE_FORMAT, BUNDLE_FORMAT_VERSION, ExportContentMode};
use crate::import::import;
use crate::import_fixture::{Harness, TempBundle, count_items, unique, write_bundle};

fn full_bundle(h: &Harness) -> TempBundle {
    write_bundle(&h.full_entities(), ExportContentMode::DsarPlaintext)
}

#[test]
fn dry_run_reports_and_leaves_store_unchanged() {
    let h = Harness::new();
    let bundle = full_bundle(&h);
    let ctx = h.ctx();
    let before = count_items(&h);
    let triples_before = h.store.list_triples(&h.bank).unwrap().len();
    let assoc_before = h.store.list_assoc_edges(&h.bank, None).unwrap().len();
    let stable_before = h
        .store
        .stable_history(&h.bank, "communication_style")
        .unwrap()
        .len();
    let failures_before = h.store.list_failures(&h.bank, 100).unwrap().len();

    let report = import(&h.args(&ctx, h.policy(), bundle.path(), true, false, None))
        .expect("dry run succeeds");

    assert!(report.dry_run);
    assert!(!report.applied);
    assert_eq!(
        (
            report.would_create,
            report.would_skip,
            report.would_reject,
            report.would_overwrite
        ),
        (11, 0, 0, 0)
    );
    assert_eq!(count_items(&h), before);
    assert_eq!(h.store.list_triples(&h.bank).unwrap().len(), triples_before);
    let beliefs = h.store.list_beliefs(&h.bank).unwrap();
    assert!(beliefs.is_empty(), "{beliefs:?}");
    assert_eq!(
        h.store.list_assoc_edges(&h.bank, None).unwrap().len(),
        assoc_before
    );
    assert_eq!(
        h.store
            .stable_history(&h.bank, "communication_style")
            .unwrap()
            .len(),
        stable_before
    );
    assert_eq!(
        h.store.list_failures(&h.bank, 100).unwrap().len(),
        failures_before
    );
    assert!(
        h.store
            .get_preference(&h.bank, "brevity_affinity")
            .unwrap()
            .is_none()
    );
}

#[test]
fn re_import_is_fully_idempotent() {
    let h = Harness::new();
    let bundle = full_bundle(&h);
    let ctx = h.ctx();
    let first = import(&h.args(&ctx, h.policy(), bundle.path(), false, false, None))
        .expect("first import succeeds");
    assert!(first.applied);
    assert_eq!(first.would_reject, 0);
    let items_after_first = count_items(&h);

    let second = import(&h.args(&ctx, h.policy(), bundle.path(), false, false, None))
        .expect("re-import succeeds");
    assert_eq!(second.would_skip, first.would_create);
    assert_eq!(second.would_create, 0);
    assert_eq!(second.would_reject, 0);
    assert_eq!(count_items(&h), items_after_first);
    assert_eq!(h.store.list_triples(&h.bank).unwrap().len(), 1);
    let beliefs = h.store.list_beliefs(&h.bank).unwrap();
    assert_eq!((beliefs.len(), beliefs[0].confidence_history.len()), (1, 1));
}

#[test]
fn admission_rejects_below_threshold_without_writes() {
    let h = Harness::new();
    let entities = h.reject_entities();
    let bundle = write_bundle(&entities, ExportContentMode::DsarPlaintext);
    let ctx = h.ctx();
    let mut env = RankingEnv::system_defaults();
    env.theta_admit = 1.0;
    let policy = AdmissionPolicy::new(&env);

    let dry = import(&h.args(&ctx, policy.clone(), bundle.path(), true, false, None))
        .expect("dry-run reject planning succeeds");
    assert_eq!(dry.would_reject, 1);
    assert_eq!(dry.would_create, 0);
    assert_eq!(dry.rejection_samples.len(), 1);
    assert_eq!(
        h.store
            .count_items(&ItemInspectFilter::new(&h.bank))
            .unwrap(),
        0
    );

    let live = import(&h.args(&ctx, policy.clone(), bundle.path(), false, false, None))
        .expect("live reject run succeeds");
    assert_eq!(live.would_reject, 1);
    assert!(
        !live.rejection_samples.is_empty(),
        "{:?}",
        live.rejection_samples
    );
    assert_eq!(
        h.store
            .count_items(&ItemInspectFilter::new(&h.bank))
            .unwrap(),
        0
    );
    assert!(
        h.admit
            .events()
            .iter()
            .any(|e| e.operation == "admit_reject")
    );
}

#[test]
fn tampered_bundle_fails_even_with_force() {
    let h = Harness::new();
    let bundle = full_bundle(&h);
    let mut value: serde_json::Value = serde_json::from_str(&bundle.text()).unwrap();
    let gist = value
        .pointer_mut("/entities/items/0/gist")
        .expect("semantic item gist present");
    *gist = serde_json::Value::String("tampered payload byte".into());
    let path = std::env::temp_dir().join(unique("am-import-tampered"));
    std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    let tampered = TempBundle(path);
    let ctx = h.ctx();
    let mut env = RankingEnv::system_defaults();
    env.theta_admit = 1.0;
    let policy = AdmissionPolicy::new(&env);

    let err = import(&h.args(
        &ctx,
        policy.clone(),
        tampered.path(),
        false,
        true,
        Some("audit"),
    ))
    .expect_err("tampered bundle rejected");
    assert!(err.message.contains("checksum mismatch"), "{err:?}");
    assert_eq!(
        h.store
            .count_items(&ItemInspectFilter::new(&h.bank))
            .unwrap(),
        0
    );
}

#[test]
fn bad_format_identity_is_rejected() {
    let h = Harness::new();
    let entities = h.full_entities();
    let ctx = h.ctx();

    let mut wrong_format: serde_json::Value =
        serde_json::from_str(&write_bundle(&entities, ExportContentMode::DsarPlaintext).text())
            .unwrap();
    wrong_format["format"] = serde_json::json!("other.export.format");
    let path = std::env::temp_dir().join(unique("am-import-badformat"));
    std::fs::write(&path, serde_json::to_vec(&wrong_format).unwrap()).unwrap();
    let err = import(&h.args(
        &ctx,
        h.policy(),
        TempBundle(path).path(),
        false,
        false,
        None,
    ))
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);

    let mut future: serde_json::Value =
        serde_json::from_str(&write_bundle(&entities, ExportContentMode::DsarPlaintext).text())
            .unwrap();
    future["format_version"] = serde_json::json!(99);
    let path = std::env::temp_dir().join(unique("am-import-future"));
    std::fs::write(&path, serde_json::to_vec(&future).unwrap()).unwrap();
    let err = import(&h.args(
        &ctx,
        h.policy(),
        TempBundle(path).path(),
        false,
        false,
        None,
    ))
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);

    let bare = serde_json::json!({
        "format": BUNDLE_FORMAT,
        "format_version": BUNDLE_FORMAT_VERSION,
        "content_mode": "dsar_plaintext",
        "entities": {},
    });
    let path = std::env::temp_dir().join(unique("am-import-nomanifest"));
    std::fs::write(&path, serde_json::to_vec(&bare).unwrap()).unwrap();
    assert!(
        import(&h.args(
            &ctx,
            h.policy(),
            TempBundle(path).path(),
            false,
            false,
            None
        ))
        .is_err()
    );
}

#[test]
fn force_without_reason_is_rejected() {
    let h = Harness::new();
    let bundle = full_bundle(&h);
    let ctx = h.ctx();
    let before = count_items(&h);
    let err = import(&h.args(&ctx, h.policy(), bundle.path(), false, true, None)).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    let err =
        import(&h.args(&ctx, h.policy(), bundle.path(), false, true, Some("  "))).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert_eq!(count_items(&h), before);
}

#[test]
fn force_overwrites_differing_items_and_keeps_triples() {
    let h = Harness::new();
    let entities = h.full_entities();
    let bundle = write_bundle(&entities, ExportContentMode::DsarPlaintext);
    let ctx = h.ctx();
    import(&h.args(&ctx, h.policy(), bundle.path(), false, false, None)).unwrap();

    // Same bundle, force with reason: the item is overwritten, the triple and
    // its carrier are left alone (bi-temporal identity is kept).
    let report = import(&h.args(
        &ctx,
        h.policy(),
        bundle.path(),
        false,
        true,
        Some("restore"),
    ))
    .expect("force import succeeds");
    assert!(report.would_overwrite >= 1);
    assert!(
        report
            .notes
            .iter()
            .any(|n| n.contains("force does not clobber triples"))
    );
    let semantic = entities.items.first().unwrap().id.clone();
    let restored = h
        .store
        .get_memory_item(&h.bank, &format!("{semantic}#{}", h.bank))
        .unwrap()
        .unwrap();
    assert_eq!(restored.gist, entities.items.first().unwrap().gist);
}

#[test]
fn ciphertext_backup_import_restores_opaque_rows() {
    let h = Harness::new();
    let entities = h.opaque_entities();
    let bundle = write_bundle(&entities, ExportContentMode::CiphertextBackup);
    let text = bundle.text();
    assert!(text.contains("content_cipher"));
    assert!(!text.contains("\"snapshot\""));
    assert!(!text.contains("\"gist\""));
    let ctx = h.ctx();
    let report = import(&h.args(&ctx, h.policy(), bundle.path(), false, false, None))
        .expect("opaque import succeeds");
    assert_eq!(report.would_create, 1);
    let opaque = &entities.opaque_items[0];
    // The exporting node's ciphertext is stored verbatim (no re-encryption).
    assert_eq!(
        h.store
            .inspect_content_ciphertext(&h.bank, &format!("{}#{}", opaque.id, h.bank))
            .unwrap()
            .as_deref(),
        Some(opaque.content_cipher.as_str())
    );
    // Every metadata column survives the restore.
    let rows = h
        .store
        .inspect_items(&ItemInspectFilter::new(&h.bank))
        .unwrap();
    let row = rows
        .iter()
        .find(|r| r.item_id == format!("{}#{}", opaque.id, h.bank))
        .expect("restored opaque row is listed");
    assert_eq!(row.kind, "semantic");
    assert_eq!(row.category.as_deref(), Some("schema"));
    assert_eq!(row.epistemic_kind, "fact");
    assert_eq!(row.confidence, Some(0.9));
    assert_eq!(row.admission_score, Some(0.8));
    assert_eq!(row.valid_from, "2026-01-01T00:00:00Z");
    assert_eq!(row.created_at, "2026-06-01T00:00:00Z");
    assert!(
        report
            .notes
            .iter()
            .any(|n| n.contains("opaque under the local KMS"))
    );
}
#[test]
fn invalid_caller_inputs_fail_closed() {
    let h = Harness::new();
    let bundle = full_bundle(&h);
    let ctx = h.ctx();

    // Empty bank in ctx.
    let empty_bank_ctx = OpContext::new(Bank::new(String::new()), Actor::Agent);
    let err = import(&h.args(
        &empty_bank_ctx,
        h.policy(),
        bundle.path(),
        true,
        false,
        None,
    ))
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::MissingBank);

    // Empty timestamp: build args with an empty timestamp via a wrapper run.
    let mut args = h.args(&ctx, h.policy(), bundle.path(), true, false, None);
    args.timestamp = "";
    let err = import(&args).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);

    // Missing source file.
    let missing = std::env::temp_dir().join(unique("am-import-missing"));
    let err = import(&h.args(
        &ctx,
        h.policy(),
        missing.to_str().unwrap(),
        true,
        false,
        None,
    ))
    .unwrap_err();
    assert!(err.message.contains("bundle read failed"), "{err:?}");

    // Non-JSON source file.
    let junk_path = std::env::temp_dir().join(unique("am-import-junk"));
    std::fs::write(&junk_path, "not json").unwrap();
    let err = import(&h.args(
        &ctx,
        h.policy(),
        TempBundle(junk_path).path(),
        true,
        false,
        None,
    ))
    .unwrap_err();
    assert!(err.message.contains("bundle JSON invalid"), "{err:?}");

    // Manifest version disagreeing with the envelope version.
    let mut mismatched: serde_json::Value = serde_json::from_str(&bundle.text()).unwrap();
    mismatched["manifest"]["format_version"] = serde_json::json!(BUNDLE_FORMAT_VERSION + 1);
    let path = std::env::temp_dir().join(unique("am-import-manifestver"));
    std::fs::write(&path, serde_json::to_vec(&mismatched).unwrap()).unwrap();
    let err =
        import(&h.args(&ctx, h.policy(), TempBundle(path).path(), true, false, None)).unwrap_err();
    assert!(err.message.contains("does not match envelope"), "{err:?}");
    assert_eq!(count_items(&h), 0);
}

#[test]
fn chunked_commit_reports_sequential_transactions() {
    let h = Harness::new();
    let entities = h.bulk_entities(51);
    let bundle = write_bundle(&entities, ExportContentMode::DsarPlaintext);
    let ctx = h.ctx();
    let report = import(&h.args(&ctx, h.policy(), bundle.path(), false, false, None))
        .expect("chunked import succeeds");
    assert_eq!(report.would_create, 51);
    assert!(
        report
            .notes
            .iter()
            .any(|n| n.contains("sequential transactions"))
    );
    assert_eq!(
        h.store
            .count_items(&ItemInspectFilter::new(&h.bank))
            .unwrap(),
        51
    );
}
