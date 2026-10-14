// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for source-`context` coverage of the bundle
//! import path: exact restoration through the admission gate, backward
//! compatibility with bundles written before the field existed, and the
//! published context validation rules failing closed.
//!
//! ## Owns
//! - Context-focused import assertions over the shared `import_fixture` harness
//!
//! ## Does not own
//! - Fixture builders (see `import_fixture.rs`)
//! - Export-side masking (see `export_context_tests.rs`)
//! - Persona-context imports (see `import_context_persona_tests.rs`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use std::sync::Arc;

use crate::bundle::{BundleEnvelope, ExportContentMode, ExportEntities};
use crate::export::{ExportArgs, ExportFilter, export};
use crate::import::import;
use crate::import_fixture::{Harness, unique, write_bundle};
use clio_store::{LocalDevKms, SqliteStore, Store};
use clio_types::{
    Actor, Bank, EpistemicKind, ErrorCode, ItemKind, MemoryItem, OpContext, SemanticCategory,
};
use serde_json::json;

/// One admit-passing semantic item with an optional descriptive context.
fn semantic_item(id: &str, bank: &str, context: Option<&str>) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: "sub-import-ctx".into(),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::parse("schema").unwrap()),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.9),
        admission_score: Some(0.8),
        source_ref: None,
        snapshot: Some(json!({"k": "v"})),
        gist: Some("The deploy must run the smoke task checklist".into()),
        valid_from: "2026-01-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2026-01-01T00:00:00Z".into(),
        tx_until: None,
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-01-01T00:00:00Z".into(),
        context: context.map(str::to_owned),
    }
}

/// One-item entity payload for `write_bundle`.
fn one_item(entities: Vec<MemoryItem>) -> ExportEntities {
    ExportEntities {
        items: entities,
        ..ExportEntities::default()
    }
}

/// Removes the exported bundle file when the test ends.
struct TempFile(std::path::PathBuf);

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[test]
fn exported_context_round_trips_into_a_fresh_bank() {
    let kms: Arc<dyn clio_store::DekProvider> = Arc::new(LocalDevKms::new());
    let source = SqliteStore::open_in_memory(kms).expect("source store");
    let source_bank = unique("ctx-src-bank");
    let id = unique("ctx-roundtrip");
    source
        .create_memory_item(
            &semantic_item(&id, &source_bank, Some("team chat")),
            "tester",
        )
        .expect("seed source item");
    let dest = std::env::temp_dir().join(format!("{id}.json"));
    let _guard = TempFile(dest.clone());
    let ctx = OpContext::new(Bank::new(source_bank), Actor::Harness);
    export(&ExportArgs {
        store: &source,
        compliance: &source,
        persona: &source,
        history: &source,
        belief: &source,
        ctx: &ctx,
        destination: dest.to_str().expect("utf-8 temp path"),
        filter: ExportFilter::default(),
        content_mode: ExportContentMode::DsarPlaintext,
        timestamp: "2026-09-20T00:00:00Z",
        subject_id: None,
        facts: None,
        backend: None,
        delivery: None,
    })
    .expect("export succeeds");

    // The exported bundle carries the exact context before any import runs.
    let text = std::fs::read_to_string(&dest).expect("bundle readable");
    let envelope: BundleEnvelope = serde_json::from_str(&text).expect("bundle parses");
    assert_eq!(envelope.entities.items.len(), 1);
    assert_eq!(
        envelope.entities.items[0].context.as_deref(),
        Some("team chat")
    );

    // A fresh store and bank restore it through the admission-gated create path.
    let h = Harness::new();
    let report = import(&h.args(
        &h.ctx(),
        h.policy(),
        dest.to_str().expect("utf-8 temp path"),
        false,
        false,
        None,
    ))
    .expect("import succeeds");
    assert_eq!(report.would_create, 1);
    let restored = h
        .store
        .get_memory_item(&h.bank, &id)
        .expect("read restored item")
        .expect("restored item exists");
    assert_eq!(restored.context.as_deref(), Some("team chat"));
}

#[test]
fn bundle_without_context_imports_unchanged() {
    let h = Harness::new();
    let id = unique("ctx-legacy");
    let bundle = write_bundle(
        &one_item(vec![semantic_item(&id, "exported-bank", None)]),
        ExportContentMode::DsarPlaintext,
    );
    assert!(
        !bundle.text().contains("\"context\""),
        "a bundle without context must not serialize the key"
    );
    let report = import(&h.args(&h.ctx(), h.policy(), bundle.path(), false, false, None))
        .expect("legacy import succeeds");
    assert_eq!(report.would_create, 1);
    let restored = h
        .store
        .get_memory_item(&h.bank, &id)
        .expect("read restored item")
        .expect("restored item exists");
    assert!(restored.context.is_none());
}

#[test]
fn oversized_context_is_rejected() {
    let h = Harness::new();
    let big = "a".repeat(4097);
    let bundle = write_bundle(
        &one_item(vec![semantic_item(
            &unique("ctx-big"),
            "exported-bank",
            Some(&big),
        )]),
        ExportContentMode::DsarPlaintext,
    );
    let err = import(&h.args(&h.ctx(), h.policy(), bundle.path(), false, false, None))
        .expect_err("oversized context rejected");
    assert_eq!(err.code, ErrorCode::OutOfRange);
}

#[test]
fn empty_context_is_rejected() {
    let h = Harness::new();
    let bundle = write_bundle(
        &one_item(vec![semantic_item(
            &unique("ctx-empty"),
            "exported-bank",
            Some(""),
        )]),
        ExportContentMode::DsarPlaintext,
    );
    let err = import(&h.args(&h.ctx(), h.policy(), bundle.path(), false, false, None))
        .expect_err("empty context rejected");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
}

#[test]
fn force_overwrite_scrubs_secret_shaped_context() {
    let h = Harness::new();
    let id = unique("ctx-force");
    let initial = write_bundle(
        &one_item(vec![semantic_item(&id, "exported-bank", Some("team chat"))]),
        ExportContentMode::DsarPlaintext,
    );
    import(&h.args(&h.ctx(), h.policy(), initial.path(), false, false, None))
        .expect("initial import succeeds");

    let secret = "api_key: sk-test-1234567890";
    let overwrite = write_bundle(
        &one_item(vec![semantic_item(&id, "exported-bank", Some(secret))]),
        ExportContentMode::DsarPlaintext,
    );
    let report = import(&h.args(
        &h.ctx(),
        h.policy(),
        overwrite.path(),
        false,
        true,
        Some("ops"),
    ))
    .expect("force import succeeds");
    assert_eq!(report.would_overwrite, 1);
    let restored = h
        .store
        .get_memory_item(&h.bank, &id)
        .expect("read restored item")
        .expect("restored item exists");
    assert_eq!(restored.context.as_deref(), Some("api_key: ****7890"));
}
