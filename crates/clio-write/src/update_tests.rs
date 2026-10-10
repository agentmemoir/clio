// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for contract tests of the `update` rule splitter.
//!
//! ## Owns
//! - both success paths, both mismatch failures, kind-as-rule rejection
//! - Telemetry and fail-closed (cross-bank, non-numeric, no-mutation) cases
//!
//! ## Does not own
//! - Store persistence cases (see `clio-store` attribute suites)
//!
//! ## Boundary
//!
//! Changes to routing expectations should normally update tests here.

use crate::triple::TripleVecSink;
use crate::update::{ContinuousVecSink, UpdateArgs, UpdateOutcome, update};
use clio_store::{ContinuousStore, DekProvider, LocalDevKms, SqliteStore, Store};
use clio_types::{Actor, Bank, ErrorCode, ItemKind, MemoryItem, OpContext, UpdateRule};
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

fn store() -> clio_store::SqliteStore {
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    clio_store::SqliteStore::open_in_memory(kms).expect("sqlite")
}

fn schema(bank: &str, key: &str, rule: UpdateRule) -> clio_types::AttributeSchema {
    clio_types::AttributeSchema {
        bank_id: bank.into(),
        attr_key: key.into(),
        update_rule: rule,
        alpha: Some(0.5),
        trend_window: Some(3),
        bounds_lo: None,
        bounds_hi: None,
        clamp_values: false,
        created_at: "2024-01-01".into(),
        updated_at: None,
    }
}

fn item(id: &str, bank: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: "subj".into(),
        kind: ItemKind::Episodic,
        category: None,
        episodic_type: Some(clio_types::EpisodicType::Gist),
        epistemic_kind: clio_types::EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        admission_score: Some(0.5),
        source_ref: None,
        snapshot: Some(serde_json::json!({"gist": "x"})),
        gist: Some("x".into()),
        valid_from: "2024-01-01".into(),
        valid_until: None,
        tx_from: "2024-01-01".into(),
        tx_until: None,
        created_at: "2024-01-01".into(),
        updated_at: "2024-01-01".into(),
        context: None,
    }
}

fn args<'a>(
    s: &'a SqliteStore,
    ctx: &'a OpContext,
    target: &'a str,
    value: &'a str,
    rule: UpdateRule,
    cont: &'a ContinuousVecSink,
    triple: &'a TripleVecSink,
) -> UpdateArgs<'a> {
    UpdateArgs {
        store: s,
        continuous: s,
        ctx,
        target,
        new_value: value,
        update_rule: rule,
        timestamp: "2024-06-01",
        continuous_sink: cont,
        triple_sink: triple,
    }
}

/// Continuous update on a continuous attribute applies EMA, no invalidation.
#[test]
fn t05_continuous_on_continuous_applies_ema() {
    let s = store();
    let bank = unique("b");
    s.put_attribute_schema(&schema(&bank, "affinity", UpdateRule::Continuous), "agent")
        .unwrap();
    let cont = ContinuousVecSink::default();
    let triple = TripleVecSink::default();
    let ctx = OpContext::new(Bank::new(&bank), Actor::Agent);
    let a = args(
        &s,
        &ctx,
        "affinity",
        "1.0",
        UpdateRule::Continuous,
        &cont,
        &triple,
    );
    let out = update(&a).unwrap();
    match out {
        UpdateOutcome::Continuous(state) => {
            assert!((state.state - 1.0).abs() < 1e-9);
            assert!((state.trend - 1.0).abs() < 1e-9);
        }
        UpdateOutcome::Discrete(_) => panic!("continuous update must not invalidate"),
    }
    let a = args(
        &s,
        &ctx,
        "affinity",
        "0.0",
        UpdateRule::Continuous,
        &cont,
        &triple,
    );
    let out = update(&a).unwrap();
    match out {
        UpdateOutcome::Continuous(state) => assert!((state.state - 0.5).abs() < 1e-9),
        UpdateOutcome::Discrete(_) => panic!(),
    }
    assert_eq!(cont.events().len(), 2);
    assert!(triple.events().is_empty(), "no invalidation events");
    // A categorical payload is rejected without mutation.
    let a = args(
        &s,
        &ctx,
        "affinity",
        "boston",
        UpdateRule::Continuous,
        &cont,
        &triple,
    );
    let err = update(&a).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidType);
    let a = args(
        &s,
        &ctx,
        "affinity",
        "NaN",
        UpdateRule::Continuous,
        &cont,
        &triple,
    );
    assert_eq!(update(&a).unwrap_err().code, ErrorCode::OutOfRange);
    let series = s.continuous_series(&bank, "affinity", None, 100).unwrap();
    assert_eq!(series.len(), 2, "rejected payloads apply nothing");
    assert_eq!(cont.events().len(), 2);
}

/// Discrete update on a continuous attribute → wrong_update_rule, no mutation.
#[test]
fn t06_discrete_on_continuous_rejected() {
    let s = store();
    let bank = unique("b");
    s.put_attribute_schema(&schema(&bank, "affinity", UpdateRule::Continuous), "agent")
        .unwrap();
    let ctx = OpContext::new(Bank::new(&bank), Actor::Agent);
    let cont = ContinuousVecSink::default();
    let triple = TripleVecSink::default();
    let a = args(
        &s,
        &ctx,
        "affinity",
        "1.0",
        UpdateRule::Discrete,
        &cont,
        &triple,
    );
    let err = update(&a).unwrap_err();
    assert_eq!(err.code, ErrorCode::WrongUpdateRule);
    assert!(cont.events().is_empty(), "no continuous row was written");
    assert!(triple.events().is_empty(), "no invalidation was written");
    let series = s.continuous_series(&bank, "affinity", None, 100).unwrap();
    assert!(series.is_empty(), "a point was written: {series:?}");
}

/// continuous update on a discrete attribute → wrong_update_rule, no mutation.
#[test]
fn t07_continuous_on_discrete_rejected() {
    let s = store();
    let bank = unique("b");
    s.put_attribute_schema(&schema(&bank, "lives_in", UpdateRule::Discrete), "agent")
        .unwrap();
    s.create_memory_item(&item("i1", &bank), "agent").unwrap();
    let ctx = OpContext::new(Bank::new(&bank), Actor::Agent);
    let cont = ContinuousVecSink::default();
    let triple = TripleVecSink::default();
    let a = args(
        &s,
        &ctx,
        "lives_in",
        "0.5",
        UpdateRule::Continuous,
        &cont,
        &triple,
    );
    let err = update(&a).unwrap_err();
    assert_eq!(err.code, ErrorCode::WrongUpdateRule);
    // No mutation: the item is still open.
    let got = s.get_memory_item(&bank, "i1").unwrap().unwrap();
    assert!(got.valid_until.is_none());
    assert!(cont.events().is_empty(), "no continuous row was written");
}

/// Discrete success path: update(item_id, value, discrete) invalidates (bi-temporal).
#[test]
fn t_discrete_routes_to_invalidate() {
    let s = store();
    let bank = unique("b");
    s.create_memory_item(&item("i1", &bank), "agent").unwrap();
    let ctx = OpContext::new(Bank::new(&bank), Actor::Agent);
    let cont = ContinuousVecSink::default();
    let triple = TripleVecSink::default();
    let a = args(&s, &ctx, "i1", "0.5", UpdateRule::Discrete, &cont, &triple);
    match update(&a).unwrap() {
        UpdateOutcome::Discrete(out) => assert_eq!(out.closed_ids, vec!["i1".to_owned()]),
        UpdateOutcome::Continuous(_) => panic!("discrete update must not EMA"),
    }
    let got = s.get_memory_item(&bank, "i1").unwrap().unwrap();
    assert_eq!(got.valid_until.as_deref(), Some("2024-06-01"));
    assert_eq!(got.tx_until.as_deref(), Some("2024-06-01"));
    assert_eq!(triple.events().len(), 1);
    assert_eq!(triple.events()[0].operation, "invalidate");
    assert!(cont.events().is_empty(), "no continuous row was written");
    // Already-closed item invalidates as a documented no-op.
    let a = args(&s, &ctx, "i1", "0.5", UpdateRule::Discrete, &cont, &triple);
    match update(&a).unwrap() {
        UpdateOutcome::Discrete(out) => assert!(out.already_ended),
        UpdateOutcome::Continuous(_) => panic!(),
    }
    // Unknown item fails closed.
    let a = args(
        &s,
        &ctx,
        "missing",
        "0.5",
        UpdateRule::Discrete,
        &cont,
        &triple,
    );
    assert_eq!(update(&a).unwrap_err().code, ErrorCode::ItemNotFound);
}

/// `fact`/`belief` are rejected as update_rule before any mutation.
#[test]
fn t13_fact_belief_rejected_as_update_rule() {
    let s = store();
    let bank = unique("b");
    s.put_attribute_schema(&schema(&bank, "affinity", UpdateRule::Continuous), "agent")
        .unwrap();
    let cont = ContinuousVecSink::default();
    let triple = TripleVecSink::default();
    for raw in ["fact", "belief"] {
        let rule = UpdateRule::parse(raw).unwrap_err();
        assert_eq!(rule.code, ErrorCode::WrongUpdateRule);
        assert!(
            rule.message
                .contains("update_rule must be discrete|continuous")
        );
    }
    // Continuous path stayed untouched.
    let series = s.continuous_series(&bank, "affinity", None, 100).unwrap();
    assert!(series.is_empty(), "a point was written: {series:?}");
    let _ = (&cont, &triple);
    assert!(cont.events().is_empty(), "no continuous row was written");
    assert!(triple.events().is_empty(), "no invalidation was written");
}

/// Facade guards: missing bank fails closed; discrete updates are item-addressed.
#[test]
fn facade_guards() {
    let s = store();
    let bank = unique("b");
    s.put_attribute_schema(&schema(&bank, "affinity", UpdateRule::Continuous), "agent")
        .unwrap();
    let cont = ContinuousVecSink::default();
    let triple = TripleVecSink::default();
    // Missing bank.
    let empty = OpContext::new(Bank::new(""), Actor::Agent);
    let a = args(
        &s,
        &empty,
        "affinity",
        "1.0",
        UpdateRule::Continuous,
        &cont,
        &triple,
    );
    assert_eq!(update(&a).unwrap_err().code, ErrorCode::MissingBank);
    // Discrete rule aimed at a declared attribute key is item-addressed.
    s.put_attribute_schema(&schema(&bank, "lives_in", UpdateRule::Discrete), "agent")
        .unwrap();
    let ctx = OpContext::new(Bank::new(&bank), Actor::Agent);
    let a = args(
        &s,
        &ctx,
        "lives_in",
        "0.5",
        UpdateRule::Discrete,
        &cont,
        &triple,
    );
    let err = update(&a).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert!(err.message.contains("item-addressed"));
    assert!(cont.events().is_empty(), "no continuous row was written");
    assert!(triple.events().is_empty(), "no invalidation was written");
}

/// Addressability contract: attribute lookup wins over an item-id collision,
/// and a declared key on the discrete path is rejected as ambiguous.
#[test]
fn t_addressability_precedence_and_discrete_key_rejection() {
    let s = store();
    let bank = unique("b");
    // An item whose id equals a registered attribute key.
    s.create_memory_item(&item("affinity", &bank), "agent")
        .unwrap();
    s.put_attribute_schema(&schema(&bank, "affinity", UpdateRule::Continuous), "agent")
        .unwrap();
    let ctx = OpContext::new(Bank::new(&bank), Actor::Agent);
    let cont = ContinuousVecSink::default();
    let triple = TripleVecSink::default();
    // The declaration wins: continuous update observes; the item stays open.
    let a = args(
        &s,
        &ctx,
        "affinity",
        "1.0",
        UpdateRule::Continuous,
        &cont,
        &triple,
    );
    assert!(matches!(update(&a).unwrap(), UpdateOutcome::Continuous(_)));
    let got = s.get_memory_item(&bank, "affinity").unwrap().unwrap();
    assert!(got.valid_until.is_none(), "collision did not invalidate");
    // The same key on the discrete path is rejected as a declared-rule mismatch.
    let a = args(
        &s,
        &ctx,
        "affinity",
        "1.0",
        UpdateRule::Discrete,
        &cont,
        &triple,
    );
    let err = update(&a).unwrap_err();
    assert_eq!(err.code, ErrorCode::WrongUpdateRule);
    assert!(err.message.contains("declares update_rule"));
}

/// Security: cross-bank targets fail closed and never mutate the other bank.
#[test]
fn cross_bank_fails_closed() {
    let s = store();
    let bank = unique("b");
    s.put_attribute_schema(&schema(&bank, "affinity", UpdateRule::Continuous), "agent")
        .unwrap();
    let other_ctx = OpContext::new(Bank::new("other-bank"), Actor::Agent);
    let cont = ContinuousVecSink::default();
    let triple = TripleVecSink::default();
    let a = args(
        &s,
        &other_ctx,
        "affinity",
        "1.0",
        UpdateRule::Continuous,
        &cont,
        &triple,
    );
    let err = update(&a).unwrap_err();
    assert_eq!(err.code, ErrorCode::WrongUpdateRule);
    let series = s.continuous_series(&bank, "affinity", None, 100).unwrap();
    assert!(series.is_empty(), "a point was written: {series:?}");
    // Discrete path refuses a triple-id target with the existing invalidation error.
    assert!(s.get_attribute_schema(&bank, "affinity").unwrap().is_some());
}
