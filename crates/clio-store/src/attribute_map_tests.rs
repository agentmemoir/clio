// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for SQLite continuous-attribute persistence tests.
//!
//! ## Owns
//! - Declaration round-trip, immutability, `epistemic_kind` column separation
//! - Appendix fixture through the store, series as-of, corrupt-window recovery, concurrency
//!
//! ## Does not own
//! - Postgres parity (see `pg_attribute_tests`)
//! - Formula math (unit-covered in `clio-types`)
//!
//! ## Boundary
//!
//! Changes to continuous persistence expectations should normally update tests here.

use crate::continuous::ContinuousStore;
use crate::dek::{DekProvider, LocalDevKms};
use crate::persona_store::PersonaStore;
use crate::sqlite::SqliteStore;
use clio_types::{AttributeSchema, DEFAULT_ALPHA, PersonaPreference, UpdateRule};
use std::sync::Arc;

use crate::test_unique::unique;

fn store() -> SqliteStore {
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    SqliteStore::open_in_memory(kms).expect("sqlite")
}

fn schema(bank: &str, key: &str, rule: UpdateRule) -> AttributeSchema {
    AttributeSchema {
        bank_id: bank.into(),
        attr_key: key.into(),
        update_rule: rule,
        alpha: None,
        trend_window: None,
        bounds_lo: None,
        bounds_hi: None,
        clamp_values: false,
        created_at: "2024-01-01".into(),
        updated_at: None,
    }
}

/// Declaration round-trip + separation from the item `epistemic_kind` column.
#[test]
fn t01_declaration_roundtrip_not_epistemic_column() {
    let s = store();
    let bank = unique("b");
    let mut sch = schema(&bank, "affinity", UpdateRule::Continuous);
    sch.alpha = Some(0.4);
    sch.trend_window = Some(3);
    s.put_attribute_schema(&sch, "agent").unwrap();
    let got = s
        .get_attribute_schema(&bank, "affinity")
        .unwrap()
        .expect("declared");
    assert_eq!(got.update_rule, UpdateRule::Continuous);
    assert_eq!(got.alpha, Some(0.4));
    assert_eq!(got.trend_window, Some(3));
    // Column separation: attribute_schemas.attr_class carries the update_rule
    // while items.epistemic_kind stays fact|belief — different table, no reuse.
    s.test_execute(&format!(
        "INSERT INTO items (id, bank_id, kind, epistemic_kind, valid_from, tx_from,
                            created_at, updated_at, content_ciphertext)
         VALUES ('i-ek', '{bank}', 'episodic', 'fact', 't', 't', 't', 't', 'x')"
    ))
    .unwrap();
    let (epistemic, class): (String, String) = s
        .with_conn(|conn| {
            let epistemic: String = conn
                .query_row(
                    "SELECT epistemic_kind FROM items WHERE id = 'i-ek'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            let class: String = conn
                .query_row(
                    "SELECT attr_class FROM attribute_schemas WHERE attr_key = 'affinity'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            Ok((epistemic, class))
        })
        .unwrap();
    assert_eq!(epistemic, "fact");
    assert_eq!(class, "continuous");
    // Bank scoping: another bank sees no declaration.
    assert!(
        s.get_attribute_schema(&unique("other"), "affinity")
            .unwrap()
            .is_none()
    );
}

/// Declaration immutability: knob refresh allowed for the same rule; rule change fails.
#[test]
fn declaration_is_immutable() {
    let s = store();
    let bank = unique("b");
    s.put_attribute_schema(&schema(&bank, "affinity", UpdateRule::Continuous), "agent")
        .unwrap();
    let err = s
        .put_attribute_schema(&schema(&bank, "affinity", UpdateRule::Discrete), "agent")
        .unwrap_err();
    assert_eq!(err.code, clio_types::ErrorCode::Forbidden);
    let mut refresh = schema(&bank, "affinity", UpdateRule::Continuous);
    refresh.alpha = Some(0.9);
    s.put_attribute_schema(&refresh, "agent").unwrap();
    assert_eq!(
        s.get_attribute_schema(&bank, "affinity")
            .unwrap()
            .unwrap()
            .alpha,
        Some(0.9)
    );
    // Empty actor and invalid knobs fail closed.
    assert!(
        s.put_attribute_schema(&schema(&bank, "x", UpdateRule::Discrete), "")
            .is_err()
    );
    let mut bad = schema(&bank, "x", UpdateRule::Continuous);
    bad.alpha = Some(2.0);
    assert_eq!(
        s.put_attribute_schema(&bad, "agent").unwrap_err().code,
        clio_types::ErrorCode::OutOfRange
    );
}

/// (store level): the appendix fixture through persistence; cold start.
#[test]
fn t02_t04_appendix_fixture_through_store() {
    let s = store();
    let bank = unique("b");
    let mut sch = schema(&bank, "affinity", UpdateRule::Continuous);
    sch.alpha = Some(0.5);
    sch.trend_window = Some(3);
    s.put_attribute_schema(&sch, "agent").unwrap();
    let fixture: [(f64, f64, f64); 3] = [(1.0, 1.0, 1.0), (0.0, 0.5, 0.75), (1.0, 0.75, 0.75)];
    for (i, (x, exp_state, exp_trend)) in fixture.into_iter().enumerate() {
        let out = s
            .observe_continuous(
                &bank,
                "affinity",
                x,
                &format!("2024-01-0{}", i + 1),
                "agent",
            )
            .unwrap();
        assert!(
            (out.state - exp_state).abs() < 1e-9,
            "t={i} state={}",
            out.state
        );
        assert!(
            (out.trend - exp_trend).abs() < 1e-9,
            "t={i} trend={}",
            out.trend
        );
    }
    let all = s.continuous_series(&bank, "affinity", None, 100).unwrap();
    assert_eq!(all.len(), 3);
    assert_eq!(all[2].as_of, "2024-01-03");
    assert!((all[2].state - 0.75).abs() < 1e-9);
    // Identical formula applied again with the declared α: 0.5·1.0 + 0.5·0.75.
    let out = s
        .observe_continuous(&bank, "affinity", 1.0, "2024-01-04", "agent")
        .unwrap();
    assert!((out.state - 0.875).abs() < 1e-9, "state={}", out.state);
    assert_eq!(out.window, vec![0.5, 0.75, 0.875]);
}

/// Append-only series retention, as-of filter, no deletion on failure.
#[test]
fn t10_series_retention_and_as_of() {
    let s = store();
    let bank = unique("b");
    s.put_attribute_schema(&schema(&bank, "affinity", UpdateRule::Continuous), "agent")
        .unwrap();
    s.observe_continuous(&bank, "affinity", 1.0, "2024-01-01", "agent")
        .unwrap();
    s.observe_continuous(&bank, "affinity", 0.0, "2024-01-02", "agent")
        .unwrap();
    let err = s
        .observe_continuous(&bank, "affinity", f64::NAN, "2024-01-03", "agent")
        .unwrap_err();
    assert_eq!(err.code, clio_types::ErrorCode::OutOfRange);
    let all = s.continuous_series(&bank, "affinity", None, 100).unwrap();
    assert_eq!(all.len(), 2, "failed observe must not delete history");
    let mid = s
        .continuous_series(&bank, "affinity", Some("2024-01-01"), 100)
        .unwrap();
    assert_eq!(mid.len(), 1);
    assert!((mid[0].state - 1.0).abs() < 1e-9);
    assert_eq!(
        s.continuous_series(&bank, "affinity", None, 1)
            .unwrap()
            .len(),
        1
    );
}

/// Undeclared key, mismatched declaration, and cross-bank observes fail closed.
#[test]
fn observe_fails_closed() {
    let s = store();
    let bank = unique("b");
    let err = s
        .observe_continuous(&bank, "nope", 1.0, "t", "agent")
        .unwrap_err();
    assert_eq!(err.code, clio_types::ErrorCode::WrongUpdateRule);
    s.put_attribute_schema(&schema(&bank, "lives_in", UpdateRule::Discrete), "agent")
        .unwrap();
    let err = s
        .observe_continuous(&bank, "lives_in", 1.0, "t", "agent")
        .unwrap_err();
    assert_eq!(err.code, clio_types::ErrorCode::WrongUpdateRule);
    s.put_attribute_schema(&schema(&bank, "affinity", UpdateRule::Continuous), "agent")
        .unwrap();
    let other = unique("other");
    let err = s
        .observe_continuous(&other, "affinity", 1.0, "t", "agent")
        .unwrap_err();
    assert_eq!(err.code, clio_types::ErrorCode::WrongUpdateRule);
    // Out-of-bounds observation rejects when the schema declares bounds.
    let mut bounded = schema(&bank, "bounded", UpdateRule::Continuous);
    bounded.bounds_lo = Some(0.0);
    bounded.bounds_hi = Some(1.0);
    s.put_attribute_schema(&bounded, "agent").unwrap();
    let err = s
        .observe_continuous(&bank, "bounded", 5.0, "t", "agent")
        .unwrap_err();
    assert_eq!(err.code, clio_types::ErrorCode::OutOfRange);
    assert!(s.get_attribute_schema(&bank, "bounded").unwrap().is_some());
}

/// Argument validation guards fail closed without touching the store.
#[test]
fn observe_and_series_validate_arguments() {
    let s = store();
    let bank = unique("b");
    s.put_attribute_schema(&schema(&bank, "affinity", UpdateRule::Continuous), "agent")
        .unwrap();
    assert_eq!(
        s.observe_continuous("", "affinity", 1.0, "t", "agent")
            .unwrap_err()
            .code,
        clio_types::ErrorCode::InvalidArgument
    );
    assert_eq!(
        s.observe_continuous(&bank, "affinity", 1.0, "", "agent")
            .unwrap_err()
            .code,
        clio_types::ErrorCode::InvalidArgument
    );
    assert_eq!(
        s.continuous_series(&bank, "", None, 10).unwrap_err().code,
        clio_types::ErrorCode::InvalidArgument
    );
    // Empty actor is rejected on the observe trust boundary.
    assert!(
        s.observe_continuous(&bank, "affinity", 1.0, "t", "")
            .is_err()
    );
    let series = s.continuous_series(&bank, "affinity", None, 100).unwrap();
    assert!(series.is_empty(), "{series:?}");
}

/// Corrupt stored window degrades to series rebuild, never a hard failure.
///
/// Default α=0.3 applies to the rebuilt prior state [1.0]: state = 0.3·0 + 0.7·1.0.
#[test]
fn corrupt_window_rebuilds_from_series() {
    let s = store();
    let bank = unique("b");
    s.put_attribute_schema(&schema(&bank, "affinity", UpdateRule::Continuous), "agent")
        .unwrap();
    s.observe_continuous(&bank, "affinity", 1.0, "2024-01-01", "agent")
        .unwrap();
    s.test_execute("UPDATE continuous_state SET window_json = 'not-json'")
        .unwrap();
    let out = s
        .observe_continuous(&bank, "affinity", 0.0, "2024-01-02", "agent")
        .unwrap();
    assert!((out.state - 0.7).abs() < 1e-9, "state={}", out.state);
    assert_eq!(out.window, vec![1.0, 0.7]);
}

/// Regression: a series point carries the key's stored preference context
/// only from the point where it became effective. Previously the store cloned
/// one current context onto every point, so a read before the context existed
/// leaked it onto earlier points.
#[test]
fn preference_context_is_effective_from_its_introduction() {
    let s = store();
    let bank = unique("b");
    s.put_attribute_schema(&schema(&bank, "affinity", UpdateRule::Continuous), "agent")
        .unwrap();
    s.observe_continuous(&bank, "affinity", 0.2, "2026-01-01", "agent")
        .unwrap();
    s.observe_continuous(&bank, "affinity", 0.8, "2026-02-01", "agent")
        .unwrap();
    let pref = PersonaPreference {
        bank_id: bank.clone(),
        key: "affinity".into(),
        value: 0.8,
        trend: 0.8,
        confidence: None,
        source: "explicit".into(),
        source_ref: None,
        context: Some("kept short after feedback".into()),
        admission_score: 0.9,
        as_of: "2026-02-01".into(),
    };
    s.put_preference_state(&pref, "seeder").unwrap();

    // Full read: the pre-context point carries none; the effective point does.
    let full = s.continuous_series(&bank, "affinity", None, 100).unwrap();
    assert_eq!(full.len(), 2);
    assert!(
        full[0].context.is_none(),
        "pre-context point must not carry it: {full:?}"
    );
    assert_eq!(
        full[1].context.as_deref(),
        Some("kept short after feedback")
    );

    // The reviewed leak: an as_of read before the context existed returns only
    // the earlier point, with no context.
    let early = s
        .continuous_series(&bank, "affinity", Some("2026-01-15"), 100)
        .unwrap();
    assert_eq!(early.len(), 1);
    assert!(
        early[0].context.is_none(),
        "pre-context point-in-time read must not leak it: {early:?}"
    );
    // A read at the context's effective time carries it.
    let late = s
        .continuous_series(&bank, "affinity", Some("2026-02-01"), 100)
        .unwrap();
    assert_eq!(late.len(), 2);
    assert_eq!(
        late[1].context.as_deref(),
        Some("kept short after feedback")
    );
}

/// Concurrent same-key observes are serialized; no lost update.
///
/// The series must hold every observation exactly once, and the retained
/// states must form a valid EMA chain over some serial permutation of the
/// submitted values (reconstruct `x_k = (s_k − (1−α)·s_{k−1}) / α`).
#[test]
fn t14_concurrent_observes_serialized() {
    let s = Arc::new(store());
    let bank = unique("b");
    s.put_attribute_schema(&schema(&bank, "affinity", UpdateRule::Continuous), "agent")
        .unwrap();
    let handles: Vec<_> = (0..8)
        .map(|i| {
            let s = Arc::clone(&s);
            let bank = bank.clone();
            std::thread::spawn(move || {
                s.observe_continuous(&bank, "affinity", f64::from(i), "2024-01-01", "agent")
            })
        })
        .collect();
    for h in handles {
        h.join().expect("observe thread").expect("observe ok");
    }
    let all = s.continuous_series(&bank, "affinity", None, 100).unwrap();
    assert_eq!(all.len(), 8, "every observation applied exactly once");
    let mut recovered: Vec<i64> = Vec::new();
    for (i, p) in all.iter().enumerate() {
        let x = if i == 0 {
            p.state
        } else {
            (p.state - (1.0 - DEFAULT_ALPHA) * all[i - 1].state) / DEFAULT_ALPHA
        };
        assert!(
            (x - x.round()).abs() < 1e-6 && (-1e-6..=7.0 + 1e-6).contains(&x),
            "reconstructed observation {x} is not one of the submitted values"
        );
        recovered.push(x.round() as i64);
    }
    recovered.sort_unstable();
    assert_eq!(recovered, (0..8).collect::<Vec<_>>(), "no lost updates");
    // A follow-up observe extends the serialized history with the default α.
    let out = s
        .observe_continuous(&bank, "affinity", 0.0, "2024-01-02", "agent")
        .unwrap();
    let prior = all.last().unwrap().state;
    assert!(
        (DEFAULT_ALPHA * 0.0 + (1.0 - DEFAULT_ALPHA) * prior - out.state).abs() < 1e-9,
        "state={}",
        out.state
    );
}
