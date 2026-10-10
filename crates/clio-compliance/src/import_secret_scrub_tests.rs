// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the import `context` scrub assertions
//! on the non-persona staging paths: a hand-crafted bundle carrying a
//! secret-shaped `context` must not persist it through the item plain-create
//! path, belief create, or task record staging. The exporter already scrubs,
//! so only a bundle that skipped the exporter can produce this input.
//!
//! ## Owns
//! - Item, belief, and task import `context` masking assertions over the shared
//!   `import_fixture` harness
//!
//! ## Does not own
//! - The persona staging paths (see `import_persona_scrub_tests.rs`)
//! - The export-side masking contract (see `export_mask_identity_tests.rs`)
//! - The `masked_item` helper itself (see `import_apply.rs`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::bundle::{ExportContentMode, ExportEntities};
use crate::import::import;
use crate::import_fixture::{Harness, unique, write_bundle};
use clio_store::{BeliefStore, HistoryStore, Store};
use clio_types::{
    BeliefObject, ConfidenceEntry, EpisodicType, EpistemicKind, ItemKind, MemoryItem,
    SemanticCategory, SourceType, TaskRecord,
};
use serde_json::json;

/// A secret-shaped `context` that only a hand-crafted bundle can carry: the
/// export scrubber would already have masked it.
const SECRET: &str = "api_key: sk-test-1234567890";

/// The value `scrub_inline_secrets` produces, spelled out independently so the
/// test does not merely restate the helper it is checking.
const SECRET_MASKED: &str = "api_key: ****7890";

/// A minimal admitted semantic item carrying `context`, built by hand so the
/// raw secret reaches the importer without passing the exporter.
fn item(id: &str, context: Option<&str>) -> MemoryItem {
    item_with_episodic(id, context, None)
}

/// A task carrier: same shape, but the importer requires
/// `episodic_type=task` on the backing item.
fn task_carrier(id: &str, context: Option<&str>) -> MemoryItem {
    item_with_episodic(id, context, Some(EpisodicType::Task))
}

fn item_with_episodic(
    id: &str,
    context: Option<&str>,
    episodic_type: Option<EpisodicType>,
) -> MemoryItem {
    // A task/failure carrier is episodic; a plain item is semantic.
    let kind = if episodic_type.is_some() {
        ItemKind::Episodic
    } else {
        ItemKind::Semantic
    };
    MemoryItem {
        id: id.into(),
        bank_id: "exported-bank".into(),
        subject_id: "sub-f06".into(),
        kind,
        category: if episodic_type.is_some() {
            None
        } else {
            Some(SemanticCategory::parse("schema").expect("valid category"))
        },
        episodic_type,
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

fn one_item(id: &str, context: Option<&str>) -> ExportEntities {
    ExportEntities {
        items: vec![item(id, context)],
        ..ExportEntities::default()
    }
}

#[test]
fn a_handcrafted_item_context_is_scrubbed_on_the_plain_create_path() {
    let h = Harness::new();
    let id = unique("f06-item-plain");
    // Built by hand, so the raw secret reaches the importer unscrubbed. The
    // force-overwrite path already masked; this covers the path that did not.
    let bundle = write_bundle(
        &one_item(&id, Some(SECRET)),
        ExportContentMode::DsarPlaintext,
    );
    assert!(
        bundle.text().contains("sk-test-1234567890"),
        "the hand-built bundle must really carry the raw secret, or this test proves nothing"
    );

    let report = import(&h.args(&h.ctx(), h.policy(), bundle.path(), false, false, None))
        .expect("import succeeds");
    assert_eq!(report.would_create, 1);

    let restored = h
        .store
        .get_memory_item(&h.bank, &id)
        .expect("read restored item")
        .expect("restored item exists");
    let context = restored.context.as_deref().expect("context present");
    assert!(
        !context.contains("sk-test-1234567890"),
        "the raw secret must not be persisted: {context}"
    );
    assert_eq!(context, SECRET_MASKED);
}

#[test]
fn an_ordinary_item_context_is_not_altered_by_the_scrub() {
    let h = Harness::new();
    let id = unique("f06-item-clean");
    let bundle = write_bundle(
        &one_item(&id, Some("team chat")),
        ExportContentMode::DsarPlaintext,
    );

    let report = import(&h.args(&h.ctx(), h.policy(), bundle.path(), false, false, None))
        .expect("import succeeds");
    assert_eq!(report.would_create, 1);

    let restored = h
        .store
        .get_memory_item(&h.bank, &id)
        .expect("read restored item")
        .expect("restored item exists");
    assert_eq!(
        restored.context.as_deref(),
        Some("team chat"),
        "a context with no secret must restore unchanged"
    );
}

fn belief_with_secret(id: &str) -> BeliefObject {
    BeliefObject {
        id: id.into(),
        bank_id: "exported-bank".into(),
        subject_id: "sub-f06".into(),
        proposition: "the deploy gate needs the smoke checklist".into(),
        context: Some(SECRET.into()),
        context_as_of: None,
        confidence_history: vec![ConfidenceEntry {
            as_of: "2026-01-01T00:00:00Z".into(),
            confidence: 0.9,
            evidence_ref: "turn_1".into(),
            source_type: SourceType::UserStated,
        }],
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-01-01T00:00:00Z".into(),
    }
}

#[test]
fn a_handcrafted_belief_context_is_scrubbed_on_create() {
    let h = Harness::new();
    let id = unique("f06-belief");
    let bundle = write_bundle(
        &ExportEntities {
            beliefs: vec![belief_with_secret(&id)],
            ..ExportEntities::default()
        },
        ExportContentMode::DsarPlaintext,
    );
    assert!(
        bundle.text().contains("sk-test-1234567890"),
        "the hand-built bundle must really carry the raw secret"
    );

    import(&h.args(&h.ctx(), h.policy(), bundle.path(), false, false, None))
        .expect("import succeeds");

    // The importer mints its own belief id, so look the row up by listing.
    let beliefs = h
        .store
        .list_beliefs(&h.bank)
        .expect("list restored beliefs");
    let restored = beliefs.first().expect("restored belief exists");
    let context = restored.context.as_deref().expect("context present");
    assert!(
        !context.contains("sk-test-1234567890"),
        "the raw secret must not be persisted on a belief: {context}"
    );
    assert_eq!(context, SECRET_MASKED);
}

/// A task record is staged as a whole clone, so its `context` is masked at the
/// same point as the belief's.
#[test]
fn a_handcrafted_task_context_is_scrubbed_on_staging() {
    let h = Harness::new();
    let item_id = unique("f06-task-item");
    let task_id = unique("f06-task");
    let record = TaskRecord {
        id: format!("{task_id}-v1"),
        bank_id: "exported-bank".into(),
        item_id: item_id.clone(),
        task_id: task_id.clone(),
        subject_id: "sub-f06".into(),
        version: 1,
        definition: "run the smoke checklist".into(),
        steps: vec![],
        status: "open".into(),
        evidence_ref: None,
        context: Some(SECRET.into()),
        admission_score: 0.8,
        created_at: "2026-01-01T00:00:00Z".into(),
    };
    let bundle = write_bundle(
        &ExportEntities {
            items: vec![task_carrier(&item_id, Some(SECRET))],
            tasks: vec![record],
            ..ExportEntities::default()
        },
        ExportContentMode::DsarPlaintext,
    );

    import(&h.args(&h.ctx(), h.policy(), bundle.path(), false, false, None))
        .expect("import succeeds");

    let restored = h
        .store
        .get_task(&h.bank, &task_id)
        .expect("read restored task")
        .expect("restored task exists");
    let context = restored.context.as_deref().expect("context present");
    assert!(
        !context.contains("sk-test-1234567890"),
        "the raw secret must not be persisted on a task record: {context}"
    );
    assert_eq!(context, SECRET_MASKED);
}
