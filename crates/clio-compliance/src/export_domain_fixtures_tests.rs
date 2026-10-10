// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module holds the domain-record `context` fixture helpers for the export
//! scenario suites: seeding one already-known `context` per record kind, reading
//! a record's exported `context` back, and revalidating emitted payloads.
//!
//! ## Owns
//! - The domain-record context seeder and the emitted-context bound check
//! - The `domain_context` reader over exported entities
//!
//! ## Does not own
//! - The base export seed data and drivers (see `export_tests.rs`)
//! - The export writer itself (see `export.rs`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::bundle::ExportEntities;
use clio_store::{BeliefStore, HistoryStore, PersonaStore, Store};
use clio_types::EpisodicType;

use super::{Fixture, belief, failure, item, pref, stable, task, triple};

/// Seeds one record of every context-bearing kind with its own distinct,
/// non-secret `context`, for the per-record-type plaintext round trip.
pub(crate) fn seed_plain_domain_record_contexts(f: &Fixture) {
    let mut stable_row = stable(&f.bank, "chatty", "2026-05-01T00:00:00Z");
    stable_row.context = Some("stable why".into());
    f.store.put_stable_entry(&stable_row, "tester").unwrap();
    let mut pref_row = pref(&f.bank);
    pref_row.context = Some("pref why".into());
    f.store.put_preference_state(&pref_row, "tester").unwrap();
    let mut belief_row = belief(&f.bank);
    belief_row.id = "belief-plain-1".into();
    belief_row.proposition = "user prefers staged releases".into();
    belief_row.context = Some("belief why".into());
    f.store.create_belief(&belief_row, "tester").unwrap();
    // Task/failure/triple reads surface their carrier item's sealed `context`.
    let (mut task_carrier, mut task_record) = task(&f.bank);
    task_carrier.id = "task-plain-1".into();
    task_carrier.context = Some("task why".into());
    task_record.item_id = task_carrier.id.clone();
    task_record.id = "tr-task-plain-1".into();
    task_record.task_id = "build-plain".into();
    f.store
        .commit_task_upsert(&task_carrier, &task_record, "tester")
        .unwrap();
    let (mut fail_carrier, mut fail_record) = failure(&f.bank);
    fail_carrier.id = "fail-plain-1".into();
    fail_carrier.context = Some("failure why".into());
    fail_record.item_id = fail_carrier.id.clone();
    fail_record.id = "fr-fail-plain-1".into();
    fail_record.task_id = "build-plain".into();
    f.store
        .commit_failure_record(&fail_carrier, &fail_record, "tester")
        .unwrap();
    let mut tri_carrier = item(
        "tri-plain-1",
        &f.bank,
        "sub-c",
        None,
        Some(EpisodicType::Triple),
        None,
    );
    tri_carrier.context = Some("triple why".into());
    let mut triple_row = triple(&f.bank);
    triple_row.id = "triple-plain-1".into();
    triple_row.item_id = tri_carrier.id.clone();
    triple_row.predicate = "mentions".into();
    triple_row.object = "chat".into();
    f.store
        .commit_triple_add(&tri_carrier, &triple_row, false, "tester")
        .unwrap();
}

/// The exported `context` of one domain record, addressed by its identity:
/// `kind` is `triple`/`task`/`failure`/`belief`/`stable`/`preference` and `id`
/// is the record id (`task_id` for tasks, `as_of` for persona stable rows).
pub(crate) fn domain_context(e: &ExportEntities, kind: &str, id: &str) -> Option<String> {
    match kind {
        "triple" => e
            .triples
            .iter()
            .find(|r| r.id == id)
            .and_then(|r| r.context.clone()),
        "task" => e
            .tasks
            .iter()
            .find(|r| r.task_id == id)
            .and_then(|r| r.context.clone()),
        "failure" => e
            .failures
            .iter()
            .find(|r| r.id == id)
            .and_then(|r| r.context.clone()),
        "belief" => e
            .beliefs
            .iter()
            .find(|r| r.id == id)
            .and_then(|r| r.context.clone()),
        "stable" => e
            .persona_stable
            .iter()
            .find(|r| r.as_of == id)
            .and_then(|r| r.context.clone()),
        "preference" => e
            .persona_preferences
            .first()
            .and_then(|r| r.context.clone()),
        other => panic!("unknown domain record kind `{other}`"),
    }
}

/// Revalidate an emitted payload: every `context` is absent or a non-empty
/// value within the published byte maximum (import validators reject the rest).
pub(crate) fn assert_emitted_contexts_are_bounded(entities: &ExportEntities) {
    let contexts = entities
        .items
        .iter()
        .map(|row| row.context.as_deref())
        .chain(entities.triples.iter().map(|row| row.context.as_deref()))
        .chain(entities.tasks.iter().map(|row| row.context.as_deref()))
        .chain(entities.failures.iter().map(|row| row.context.as_deref()))
        .chain(entities.beliefs.iter().map(|row| row.context.as_deref()))
        .chain(
            entities
                .persona_stable
                .iter()
                .map(|row| row.context.as_deref()),
        )
        .chain(
            entities
                .persona_preferences
                .iter()
                .map(|row| row.context.as_deref()),
        );
    for context in contexts.flatten() {
        assert!(!context.is_empty(), "an emitted context must not be empty");
        assert!(
            context.len() <= clio_types::CONTEXT_MAX_BYTES,
            "an emitted context must fit the published maximum, got {} bytes",
            context.len()
        );
    }
}
