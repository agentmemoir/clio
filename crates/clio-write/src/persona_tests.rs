// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for tests of the persona continuous-observe adapter.
//!
//! ## Owns
//! - the thin adapter stub calls the shared engine (single invocation path)
//! - Adapter telemetry and fail-closed contract
//!
//! ## Does not own
//! - Facade routing cases (see `update_tests`)
//!
//! ## Boundary
//!
//! Changes to adapter expectations should normally update tests here.

use crate::persona::observe_continuous;
use crate::update::{ContinuousVecSink, UpdateArgs, UpdateOutcome, update};
use clio_store::{ContinuousStore, DekProvider, LocalDevKms};
use clio_types::{Actor, AttributeSchema, Bank, OpContext, UpdateRule};
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

fn schema(bank: &str, key: &str, rule: UpdateRule) -> AttributeSchema {
    AttributeSchema {
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

/// The adapter stub invokes the shared engine and emits one telemetry event.
#[test]
fn t12_adapter_calls_shared_engine() {
    let s = store();
    let bank = unique("b");
    s.put_attribute_schema(&schema(&bank, "affinity", UpdateRule::Continuous), "agent")
        .unwrap();
    let sink = ContinuousVecSink::default();
    let ctx = OpContext::new(Bank::new(&bank), Actor::Agent);
    let state = observe_continuous(&s, &ctx, "affinity", 1.0, "2024-01-01", &sink).unwrap();
    assert!((state.state - 1.0).abs() < 1e-9);
    // Second observation follows the shared engine math (α=0.5).
    let state = observe_continuous(&s, &ctx, "affinity", 0.0, "2024-01-02", &sink).unwrap();
    assert!((state.state - 0.5).abs() < 1e-9);
    assert!((state.trend - 0.75).abs() < 1e-9);
    let events = sink.events();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].operation, "observe_continuous");
    assert_eq!(events[0].attr_key, "affinity");
    assert_eq!(events[0].bank, bank);
    assert_eq!(events[0].actor, "agent");
    // Single invocation path: adapter and direct store path run one engine —
    // replaying the same observations through the store directly gives the
    // identical state/trend series.
    let other = unique("other");
    s.put_attribute_schema(&schema(&other, "affinity", UpdateRule::Continuous), "agent")
        .unwrap();
    let d1 = s
        .observe_continuous(&other, "affinity", 1.0, "2024-01-01", "agent")
        .unwrap();
    let d2 = s
        .observe_continuous(&other, "affinity", 0.0, "2024-01-02", "agent")
        .unwrap();
    // Deterministic engine: bit-identical results for identical inputs.
    assert_eq!(d2.window, state.window);
    assert!(
        (d2.state - state.state).abs() < 1e-12 && (d2.trend - state.trend).abs() < 1e-12,
        "adapter and direct store path must match: {} vs {}",
        d2.state,
        state.state
    );
    let _ = d1;
}

/// Undeclared keys and mismatched declarations fail closed; no telemetry.
#[test]
fn adapter_fails_closed() {
    let s = store();
    let bank = unique("b");
    let sink = ContinuousVecSink::default();
    let other = OpContext::new(Bank::new("other"), Actor::Agent);
    assert!(observe_continuous(&s, &other, "affinity", 1.0, "t", &sink).is_err());
    s.put_attribute_schema(&schema(&bank, "affinity", UpdateRule::Discrete), "agent")
        .unwrap();
    let ctx = OpContext::new(Bank::new(&bank), Actor::Agent);
    assert!(observe_continuous(&s, &ctx, "affinity", 1.0, "t", &sink).is_err());
    assert!(sink.events().is_empty(), "a rejected observe wrote a point");
    // The generic update facade routes continuous updates through the adapter
    // (declarations are immutable, so a fresh key carries the continuous rule).
    s.put_attribute_schema(&schema(&bank, "mood", UpdateRule::Continuous), "agent")
        .unwrap();
    let out = update(&UpdateArgs {
        store: &s,
        continuous: &s,
        ctx: &ctx,
        target: "mood",
        new_value: "0.8",
        update_rule: UpdateRule::Continuous,
        new_item_id: "itm-persona-replacement",
        timestamp: "2024-02-01",
        continuous_sink: &sink,
        triple_sink: &crate::triple::TripleVecSink::default(),
    })
    .unwrap();
    match out {
        UpdateOutcome::Continuous(state) => assert!((state.state - 0.8).abs() < 1e-9),
        UpdateOutcome::Discrete(_) => panic!("continuous update must not invalidate"),
    }
    assert_eq!(sink.events().len(), 1);
    assert_eq!(sink.events()[0].operation, "observe_continuous");
}
