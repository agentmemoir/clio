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
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use std::sync::Arc;

use crate::bundle::{BundleEnvelope, ExportContentMode, ExportEntities};
use crate::export::{ExportArgs, ExportFilter, export};
use crate::import::import;
use crate::import_fixture::{Harness, unique, write_bundle};
use clio_store::{LocalDevKms, PersonaStore, SqliteStore, Store};
use clio_types::{
    Actor, Bank, EpistemicKind, ErrorCode, ItemKind, MemoryItem, OpContext, PersonaPreference,
    PersonaStableEntry, SemanticCategory,
};
use serde_json::json;

const TS: &str = "2026-06-01T00:00:00Z";

/// A seeded persona stable row (bank-local) and the matching bundle row with a
/// different `context` but identical `key`/`value`/`as_of`.
fn stable_pair(bank: &str) -> (PersonaStableEntry, PersonaStableEntry) {
    let seeded = PersonaStableEntry {
        context: Some("old why".into()),
        bank_id: bank.to_owned(),
        key: "communication_style".into(),
        value: "concise".into(),
        confidence: Some(0.8),
        source: "explicit".into(),
        source_ref: Some("turn_1".into()),
        admission_score: 0.8,
        as_of: TS.into(),
        valid_until: None,
    };
    let mut bundled = seeded.clone();
    bundled.bank_id = "exported-bank".into();
    bundled.context = Some("new why".into());
    (seeded, bundled)
}

/// A seeded persona preference row and the matching bundle row with a
/// different `context` but identical `value`/`trend`/`as_of`.
fn preference_pair(bank: &str) -> (PersonaPreference, PersonaPreference) {
    let seeded = PersonaPreference {
        context: Some("old pref why".into()),
        bank_id: bank.to_owned(),
        key: "brevity_affinity".into(),
        value: 0.7,
        trend: 0.6,
        confidence: None,
        source: "explicit".into(),
        source_ref: None,
        admission_score: 0.7,
        as_of: TS.into(),
    };
    let mut bundled = seeded.clone();
    bundled.bank_id = "exported-bank".into();
    bundled.context = Some("new pref why".into());
    (seeded, bundled)
}

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

#[test]
fn differing_stable_context_import_applies_under_force() {
    let h = Harness::new();
    let (seeded, bundled) = stable_pair(&h.bank);
    h.store.put_stable_entry(&seeded, "tester").unwrap();
    let bundle = write_bundle(
        &ExportEntities {
            persona_stable: vec![bundled],
            ..ExportEntities::default()
        },
        ExportContentMode::DsarPlaintext,
    );

    let report = import(&h.args(
        &h.ctx(),
        h.policy(),
        bundle.path(),
        false,
        true,
        Some("restore context"),
    ))
    .expect("force import succeeds");
    assert_eq!(
        report.would_overwrite, 1,
        "differing context must overwrite"
    );

    let history = h
        .store
        .stable_history(&h.bank, "communication_style")
        .unwrap();
    assert!(
        history
            .iter()
            .any(|e| e.context.as_deref() == Some("new why")),
        "the bundled context must be preserved through import"
    );
}

#[test]
fn differing_preference_context_import_applies_under_force() {
    let h = Harness::new();
    let (seeded, bundled) = preference_pair(&h.bank);
    h.store.put_preference_state(&seeded, "tester").unwrap();
    let bundle = write_bundle(
        &ExportEntities {
            persona_preferences: vec![bundled],
            ..ExportEntities::default()
        },
        ExportContentMode::DsarPlaintext,
    );

    let report = import(&h.args(
        &h.ctx(),
        h.policy(),
        bundle.path(),
        false,
        true,
        Some("restore context"),
    ))
    .expect("force import succeeds");
    assert_eq!(
        report.would_overwrite, 1,
        "differing context must overwrite"
    );

    let stored = h
        .store
        .get_preference(&h.bank, "brevity_affinity")
        .unwrap()
        .expect("preference exists");
    assert_eq!(stored.context.as_deref(), Some("new pref why"));
}

#[test]
fn differing_context_import_without_force_preserves_stored_context() {
    let h = Harness::new();
    let (seeded, bundled) = stable_pair(&h.bank);
    h.store.put_stable_entry(&seeded, "tester").unwrap();
    let bundle = write_bundle(
        &ExportEntities {
            persona_stable: vec![bundled],
            ..ExportEntities::default()
        },
        ExportContentMode::DsarPlaintext,
    );
    let report = import(&h.args(&h.ctx(), h.policy(), bundle.path(), false, false, None))
        .expect("non-force import succeeds");
    assert_eq!(report.would_skip, 1);
    let history = h
        .store
        .stable_history(&h.bank, "communication_style")
        .unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].context.as_deref(), Some("old why"));

    let h2 = Harness::new();
    let (seeded_pref, bundled_pref) = preference_pair(&h2.bank);
    h2.store
        .put_preference_state(&seeded_pref, "tester")
        .unwrap();
    let bundle_forced = write_bundle(
        &ExportEntities {
            persona_preferences: vec![bundled_pref],
            ..ExportEntities::default()
        },
        ExportContentMode::DsarPlaintext,
    );
    let report_forced = import(&h2.args(
        &h2.ctx(),
        h2.policy(),
        bundle_forced.path(),
        false,
        false,
        None,
    ))
    .expect("non-force import succeeds");
    assert_eq!(report_forced.would_skip, 1);
    let stored = h2
        .store
        .get_preference(&h2.bank, "brevity_affinity")
        .unwrap()
        .expect("preference exists");
    assert_eq!(stored.context.as_deref(), Some("old pref why"));
}

#[test]
fn identical_context_import_stays_idempotent() {
    let h = Harness::new();
    let (seeded, mut bundled) = stable_pair(&h.bank);
    bundled.context.clone_from(&seeded.context);
    h.store.put_stable_entry(&seeded, "tester").unwrap();
    let bundle = write_bundle(
        &ExportEntities {
            persona_stable: vec![bundled],
            ..ExportEntities::default()
        },
        ExportContentMode::DsarPlaintext,
    );
    let report = import(&h.args(
        &h.ctx(),
        h.policy(),
        bundle.path(),
        false,
        true,
        Some("restore"),
    ))
    .expect("identical-context force import succeeds");
    assert_eq!(report.would_skip, 1, "identical row must stay deduplicated");
    assert_eq!(report.would_overwrite, 0);
    let history = h
        .store
        .stable_history(&h.bank, "communication_style")
        .unwrap();
    assert_eq!(history.len(), 1, "no duplicate version may be created");

    let h2 = Harness::new();
    let (seeded_pref, mut bundled_pref) = preference_pair(&h2.bank);
    bundled_pref.context.clone_from(&seeded_pref.context);
    h2.store
        .put_preference_state(&seeded_pref, "tester")
        .unwrap();
    let bundle_forced = write_bundle(
        &ExportEntities {
            persona_preferences: vec![bundled_pref],
            ..ExportEntities::default()
        },
        ExportContentMode::DsarPlaintext,
    );
    let report_forced = import(&h2.args(
        &h2.ctx(),
        h2.policy(),
        bundle_forced.path(),
        false,
        true,
        Some("restore"),
    ))
    .expect("identical-context force import succeeds");
    assert_eq!(report_forced.would_skip, 1);
    assert_eq!(report_forced.would_overwrite, 0);
}
