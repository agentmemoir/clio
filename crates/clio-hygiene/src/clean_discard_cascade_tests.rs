// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the hygiene clean-to-cascade routing test:
//! hygiene `discard` (and the `delete` alias) removes items through the same
//! store `discard_item` call as a direct discard, so the operations-removal
//! cascade reaches distilled units through one hook.
//!
//! ## Owns
//!
//! Test-only. Never compiled into production builds.
//!
//! ## Does not own
//! - The hygiene clean runner (`crate::clean`)
//! - The store discard cascade (`clio-store`)
//! - The direct discard entry point (`clio-write`)
//!
//! ## Boundary
//!
//! Changes to how hygiene removal reaches the store cascade should normally
//! require changes here. Changes to the cascade itself should not require
//! changes here.
//!
//! Keep this module focused on the hygiene-to-cascade routing tests.

use crate::clean::{CleanAction, CleanArgs, CleanTarget, clean};
use clio_store::{
    DistilledUnitRecord, DistilledUnitRecord as Record, LocalDevKms, SqliteStore, Store,
};
use clio_types::{Actor, Bank, EpistemicKind, ItemKind, MemoryItem, OpContext, SemanticCategory};
use std::sync::Arc;

fn store() -> Arc<SqliteStore> {
    SqliteStore::open_in_memory(Arc::new(LocalDevKms::new()))
        .map(Arc::new)
        .expect("open sqlite")
}

fn item(id: &str, bank: &str, subject: &str, gist: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: subject.into(),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::ToolConfig),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.8),
        admission_score: Some(0.5),
        source_ref: Some("turn_1".into()),
        snapshot: None,
        gist: Some(gist.into()),
        valid_from: "t0".into(),
        valid_until: None,
        tx_from: "t0".into(),
        tx_until: None,
        created_at: "t0".into(),
        updated_at: "t0".into(),
        context: None,
    }
}

fn distilled_item(
    bank: &str,
    unit: &str,
    subject: &str,
    hub: &str,
    sources: &[&str],
) -> MemoryItem {
    let mut unit_item = item(unit, bank, subject, "unit gist");
    unit_item.source_ref = Some("hub_distilled".into());
    unit_item.snapshot = Some(serde_json::json!({
        "hub_id": hub,
        "neighbor_ids": sources,
        "neighbor_count": sources.len(),
    }));
    unit_item
}

fn ctx(bank: &str) -> OpContext {
    OpContext::new(Bank::new(bank), Actor::parse("agent").expect("actor"))
}

/// Hygiene `discard` (action label `delete` resolves to the same action)
/// cascades: the distilled unit claiming the target drops it and goes stale.
#[test]
fn hygiene_discard_cascades_through_the_store_hook() {
    let s = store();
    let bank = "bank-hyg-cascade";
    let subject = "subj-shared";
    for id in ["hub", "n1"] {
        s.create_memory_item(&item(id, bank, subject, &format!("{id} gist")), "actor")
            .unwrap();
    }
    let unit = format!("hub-distill:{bank}:hub");
    let record = Record::new(bank, &unit, "hub", ["n1".to_owned()], "hub_distilled", "t0");
    s.commit_distilled_unit(
        &distilled_item(bank, &unit, subject, "hub", &["n1"]),
        &record,
        "actor",
    )
    .unwrap();

    let targets = [CleanTarget {
        item_id: "n1".into(),
        noise_score: Some(0.9),
        suggested_action: Some("discard".into()),
    }];
    let outcome = clean(&CleanArgs {
        store: s.as_ref(),
        hygiene: s.as_ref(),
        ctx: &ctx(bank),
        targets: &targets,
        action: CleanAction::parse("delete").expect("delete alias"),
        confirm: true,
        reason: "hygiene noise cleanup",
        timestamp: "2026-09-20T00:00:00Z",
        batch_id: "batch-1",
        note: None,
    })
    .unwrap();
    assert_eq!(outcome.applied, vec!["n1".to_owned()]);

    // One hook: the cascade the direct discard path runs also ran here.
    let after = s.get_distilled_unit(bank, &unit).unwrap().unwrap();
    assert_eq!(after.source_ids, Vec::<String>::new());
    assert_eq!(after.stale_reason.as_deref(), Some("source_removed"));
    let snapshot = s.get_snapshot(bank, &unit).unwrap().unwrap();
    assert_eq!(snapshot["stale_reason"], "source_removed");
    assert!(
        s.get_memory_item(bank, &unit)
            .unwrap()
            .unwrap()
            .gist
            .is_some(),
        "the audited derived row stays readable"
    );
}

/// A hygiene dry-run (confirm=false) cascades to nothing.
#[test]
fn hygiene_dry_run_cascades_to_nothing() {
    let s = store();
    let bank = "bank-hyg-dry";
    let subject = "subj-shared";
    s.create_memory_item(&item("hub", bank, subject, "hub gist"), "actor")
        .unwrap();
    s.create_memory_item(&item("n1", bank, subject, "n1 gist"), "actor")
        .unwrap();
    let unit = format!("hub-distill:{bank}:hub");
    let record =
        DistilledUnitRecord::new(bank, &unit, "hub", ["n1".to_owned()], "hub_distilled", "t0");
    s.commit_distilled_unit(
        &distilled_item(bank, &unit, subject, "hub", &["n1"]),
        &record,
        "actor",
    )
    .unwrap();

    let targets = [CleanTarget {
        item_id: "n1".into(),
        noise_score: Some(0.9),
        suggested_action: Some("discard".into()),
    }];
    let outcome = clean(&CleanArgs {
        store: s.as_ref(),
        hygiene: s.as_ref(),
        ctx: &ctx(bank),
        targets: &targets,
        action: CleanAction::Discard,
        confirm: false,
        reason: "hygiene noise cleanup",
        timestamp: "2026-09-20T00:00:00Z",
        batch_id: "batch-1",
        note: None,
    })
    .unwrap();
    assert!(outcome.dry_run);
    assert_eq!(outcome.would_discard, 1);
    let after = s.get_distilled_unit(bank, &unit).unwrap().unwrap();
    assert_eq!(after, record, "the dry-run must not touch the unit");
}
