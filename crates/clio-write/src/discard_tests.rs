// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for contract tests of the in-process `discard` tool.
//!
//! ## Owns
//! - dry-run zero-write, confirmed telemetry, and dry-run event silence
//! - Argument/not-found/bank-isolation failures and invalidate independence
//! - Episodic items (NULL category) and dry-run rejection of already-discarded items
//!
//! ## Does not own
//! - Store SQL and backend parity (see `clio-store` discard suites)
//!
//! ## Boundary
//!
//! Changes to discard validation, confirmation, or telemetry should normally update tests here.

use crate::discard::{DiscardArgs, DiscardOutcome, DiscardVecSink, discard};
use crate::triple::{TripleVecSink, invalidate};
use clio_store::{DekProvider, LocalDevKms, SqliteStore, Store};
use clio_types::{Actor, Bank, ErrorCode, ItemKind, MemoryItem, OpContext};
use std::sync::Arc;

/// Calls served so far in this process, so `unique` never repeats.
static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
use std::sync::atomic::Ordering;

fn unique(prefix: &str) -> String {
    // The counter is what makes this distinct: the process id is constant
    // inside one process, so clock-plus-pid alone repeats whenever two calls
    // land in the same tick, and this name is a bank id other tests share.
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("{prefix}-{n}-{}-{sequence}", std::process::id())
}

fn store() -> SqliteStore {
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    SqliteStore::open_in_memory(kms).expect("sqlite")
}

fn item(id: &str, bank: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: "subj".into(),
        kind: ItemKind::Semantic,
        category: Some(clio_types::SemanticCategory::ToolConfig),
        episodic_type: None,
        epistemic_kind: clio_types::EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.8),
        admission_score: Some(0.5),
        source_ref: None,
        snapshot: Some(serde_json::json!({"port": 8080})),
        gist: Some("service listens on 8080".into()),
        valid_from: "2024-01-01".into(),
        valid_until: None,
        tx_from: "2024-01-01".into(),
        tx_until: None,
        created_at: "2024-01-01".into(),
        updated_at: "2024-01-01".into(),
        context: None,
    }
}

/// An episodic item has `category = NULL` end to end.
fn episodic_item(id: &str, bank: &str) -> MemoryItem {
    let mut it = item(id, bank);
    it.kind = ItemKind::Episodic;
    it.category = None;
    it.episodic_type = Some(clio_types::EpisodicType::Triple);
    it.confidence = None;
    it.snapshot = Some(serde_json::json!({"subject": "svc", "predicate": "started"}));
    it
}

fn args<'a>(
    s: &'a SqliteStore,
    ctx: &'a OpContext,
    item_id: &'a str,
    reason: &'a str,
    confirm: bool,
    sink: &'a DiscardVecSink,
) -> DiscardArgs<'a> {
    DiscardArgs {
        store: s,
        ctx,
        item_id,
        reason,
        confirm,
        timestamp: "2024-06-01T00:00:00Z",
        sink,
    }
}

/// Unconfirmed discard is a zero-write, no-telemetry dry-run.
#[test]
fn dry_run_writes_nothing() {
    let s = store();
    let bank = unique("b");
    s.create_memory_item(&item("i1", &bank), "agent").unwrap();
    let ctx = OpContext::new(Bank::new(&bank), Actor::Agent);
    let sink = DiscardVecSink::default();
    let out = discard(&args(&s, &ctx, "i1", "ops cleanup", false, &sink)).unwrap();
    match out {
        DiscardOutcome::DryRun(identity) => {
            assert_eq!(identity.item_id, "i1");
            assert_eq!(identity.category.as_deref(), Some("tool_config"));
            assert_eq!(identity.epistemic_kind, "fact");
        }
        DiscardOutcome::Discarded(_) => panic!("unconfirmed discard must not write"),
    }
    assert!(sink.events().is_empty(), "dry-run must not emit telemetry");
    // The item is still open: a confirmed discard succeeds afterwards.
    assert!(matches!(
        discard(&args(&s, &ctx, "i1", "ops cleanup", true, &sink)).unwrap(),
        DiscardOutcome::Discarded(_)
    ));
}

/// A dry-run discard must not cascade either: a distilled unit claiming the
/// target keeps its claim set and stays current.
#[test]
fn dry_run_cascades_to_nothing() {
    let s = store();
    let bank = unique("b");
    s.create_memory_item(&item("i1", &bank), "agent").unwrap();
    let ctx = OpContext::new(Bank::new(&bank), Actor::Agent);
    let sink = DiscardVecSink::default();
    // One distilled unit claims i1: the cascade shape a confirmed discard
    // must propagate and a dry-run must not touch.
    let unit_id = format!("hub-distill:{bank}:hub");
    let mut unit = item(&unit_id, &bank);
    unit.subject_id = "subj".into();
    unit.source_ref = Some("hub_distilled".into());
    unit.snapshot =
        Some(serde_json::json!({"hub_id": "hub", "neighbor_ids": ["i1"], "neighbor_count": 1}));
    let record = clio_store::DistilledUnitRecord::new(
        &bank,
        &unit_id,
        "hub",
        ["i1".to_owned()],
        "hub_distilled",
        "2024-06-01T00:00:00Z",
    );
    s.commit_distilled_unit(&unit, &record, "agent").unwrap();

    let out = discard(&args(&s, &ctx, "i1", "ops cleanup", false, &sink)).unwrap();
    assert!(matches!(out, DiscardOutcome::DryRun(_)));
    let after = s.get_distilled_unit(&bank, &unit_id).unwrap().unwrap();
    assert_eq!(after, record, "the dry-run must not touch the claim set");
    assert_eq!(after.stale_reason, None, "the unit stays current");

    // The confirmed discard does cascade (store contract; see clio-store's
    // cascade suite for the full shape).
    discard(&args(&s, &ctx, "i1", "ops cleanup", true, &sink)).unwrap();
    let cascaded = s.get_distilled_unit(&bank, &unit_id).unwrap().unwrap();
    assert_eq!(cascaded.stale_reason.as_deref(), Some("source_removed"));
    assert_eq!(cascaded.source_ids, Vec::<String>::new());
}

/// A confirmed discard emits exactly one PII-safe event.
#[test]
fn confirmed_emits_one_pii_safe_event() {
    let s = store();
    let bank = unique("b");
    s.create_memory_item(&item("i1", &bank), "agent").unwrap();
    let ctx = OpContext::new(Bank::new(&bank), Actor::Agent);
    let sink = DiscardVecSink::default();
    let secret_reason = "ops cleanup for user jane@example.com";
    let out = discard(&args(&s, &ctx, "i1", secret_reason, true, &sink)).unwrap();
    assert!(matches!(out, DiscardOutcome::Discarded(_)));
    let events = sink.events();
    assert_eq!(events.len(), 1);
    let event = &events[0];
    assert_eq!(event.operation, "discard");
    assert_eq!(event.bank, bank);
    assert_eq!(event.actor, "agent");
    assert_eq!(event.timestamp, "2024-06-01T00:00:00Z");
    assert_eq!(event.item_id, "i1");
    assert_eq!(event.category.as_deref(), Some("tool_config"));
    assert_eq!(event.epistemic_kind, "fact");
    assert_eq!(event.reason_len, secret_reason.chars().count());
    assert!(
        !format!("{event:?}").contains("jane@example.com"),
        "telemetry must never carry the reason text"
    );
    // The removal is real: a repeat discard is rejected.
    assert_eq!(
        discard(&args(&s, &ctx, "i1", "again", true, &sink))
            .unwrap_err()
            .code,
        ErrorCode::InvalidArgument
    );
    assert_eq!(sink.events().len(), 1, "rejected repeat emits no event");
}

/// Validation and not-found failures are structured and write nothing.
#[test]
fn validation_and_not_found() {
    let s = store();
    let bank = unique("b");
    s.create_memory_item(&item("i1", &bank), "agent").unwrap();
    let sink = DiscardVecSink::default();

    let empty = OpContext::new(Bank::new(""), Actor::Agent);
    assert_eq!(
        discard(&args(&s, &empty, "i1", "r", true, &sink))
            .unwrap_err()
            .code,
        ErrorCode::MissingBank
    );

    let ctx = OpContext::new(Bank::new(&bank), Actor::Agent);
    assert_eq!(
        discard(&args(&s, &ctx, "  ", "r", true, &sink))
            .unwrap_err()
            .code,
        ErrorCode::InvalidArgument
    );
    assert_eq!(
        discard(&args(&s, &ctx, "i1", "   ", true, &sink))
            .unwrap_err()
            .code,
        ErrorCode::InvalidArgument
    );
    // Unknown and cross-bank targets are not found, with no leak.
    assert_eq!(
        discard(&args(&s, &ctx, "missing", "r", false, &sink))
            .unwrap_err()
            .code,
        ErrorCode::ItemNotFound
    );
    assert_eq!(
        discard(&args(&s, &ctx, "missing", "r", true, &sink))
            .unwrap_err()
            .code,
        ErrorCode::ItemNotFound
    );
    let other = OpContext::new(Bank::new("other-bank"), Actor::Agent);
    assert_eq!(
        discard(&args(&s, &other, "i1", "r", true, &sink))
            .unwrap_err()
            .code,
        ErrorCode::ItemNotFound
    );
    assert!(sink.events().is_empty(), "failed calls emit no telemetry");
    // Bank A's item is untouched by the cross-bank attempt.
    assert!(matches!(
        discard(&args(&s, &ctx, "i1", "r", false, &sink)).unwrap(),
        DiscardOutcome::DryRun(_)
    ));
}

/// A discarded item no longer resolves for the dry-run path: the preview
/// rejects it instead of reporting success on an already-discarded item.
#[test]
fn dry_run_rejects_already_discarded() {
    let s = store();
    let bank = unique("b");
    s.create_memory_item(&item("i1", &bank), "agent").unwrap();
    let ctx = OpContext::new(Bank::new(&bank), Actor::Agent);
    let sink = DiscardVecSink::default();
    assert!(matches!(
        discard(&args(&s, &ctx, "i1", "ops", true, &sink)).unwrap(),
        DiscardOutcome::Discarded(_)
    ));
    assert_eq!(
        discard(&args(&s, &ctx, "i1", "ops", false, &sink))
            .unwrap_err()
            .code,
        ErrorCode::ItemNotFound
    );
    assert_eq!(
        discard(&args(&s, &ctx, "i1", "ops", true, &sink))
            .unwrap_err()
            .code,
        ErrorCode::InvalidArgument
    );
    assert_eq!(sink.events().len(), 1, "rejections emit no telemetry");
}

/// Episodic items (NULL category) flow through the tool with a NULL category
/// in both the dry-run identity and the telemetry event.
#[test]
fn episodic_discard_reports_null_category() {
    let s = store();
    let bank = unique("b");
    s.create_memory_item(&episodic_item("e1", &bank), "agent")
        .unwrap();
    let ctx = OpContext::new(Bank::new(&bank), Actor::Agent);
    let sink = DiscardVecSink::default();
    match discard(&args(&s, &ctx, "e1", "ops cleanup", false, &sink)).unwrap() {
        DiscardOutcome::DryRun(identity) => {
            assert_eq!(identity.category, None, "episodic category must be NULL");
        }
        DiscardOutcome::Discarded(_) => panic!("unconfirmed discard must not write"),
    }
    match discard(&args(&s, &ctx, "e1", "ops cleanup", true, &sink)).unwrap() {
        DiscardOutcome::Discarded(identity) => {
            assert_eq!(identity.category, None);
        }
        DiscardOutcome::DryRun(_) => panic!("confirmed discard must write"),
    }
    let events = sink.events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].category, None);
    assert_eq!(events[0].epistemic_kind, "fact");
}

/// invalidate and discard compose without aliasing each other.
#[test]
fn invalidate_and_discard_are_independent() {
    use clio_types::UpdateRule;
    let s = store();
    let bank = unique("b");
    s.create_memory_item(&item("i1", &bank), "agent").unwrap();
    s.create_memory_item(&item("i2", &bank), "agent").unwrap();
    let ctx = OpContext::new(Bank::new(&bank), Actor::Agent);
    let sink = DiscardVecSink::default();
    let triple = TripleVecSink::default();

    // Discard then invalidate: discard left the time axes open.
    assert!(matches!(
        discard(&args(&s, &ctx, "i1", "ops", true, &sink)).unwrap(),
        DiscardOutcome::Discarded(_)
    ));
    let out = invalidate(
        &s,
        &ctx,
        "i1",
        None,
        "2024-06-02T00:00:00Z",
        Some(UpdateRule::Discrete),
        &triple,
    )
    .unwrap();
    assert!(!out.already_ended, "discard must not close the time axes");
    let got = s.get_memory_item(&bank, "i1").unwrap().unwrap();
    assert_eq!(got.valid_until.as_deref(), Some("2024-06-02T00:00:00Z"));

    // Invalidate then discard: an invalidated item can still be removed.
    invalidate(
        &s,
        &ctx,
        "i2",
        None,
        "2024-06-03T00:00:00Z",
        Some(UpdateRule::Discrete),
        &triple,
    )
    .unwrap();
    assert!(matches!(
        discard(&args(&s, &ctx, "i2", "ops", true, &sink)).unwrap(),
        DiscardOutcome::Discarded(_)
    ));
    assert_eq!(sink.events().len(), 2);
    assert_eq!(triple.events().len(), 2, "discard emits no triple event");
}
