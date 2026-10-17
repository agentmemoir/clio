// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for Postgres parity tests of continuous attributes.
//!
//! ## Owns
//! - fixture parity against SQLite, declaration immutability on Postgres
//! - same-key concurrency across two Postgres clients (advisory-lock RMW)
//!
//! ## Does not own
//! - SQLite continuous behavior (see `attribute_tests`)
//! - Formula math (unit-covered in `clio-types`)
//!
//! ## Boundary
//!
//! Changes to Postgres continuous expectations should normally update tests here.

use crate::continuous::ContinuousStore;
use crate::db_url::postgres_url_or_default;
use crate::dek::{DekProvider, LocalDevKms};
use crate::persona_store::PersonaStore;
use crate::pg_lock::lock_pg_test;
use crate::postgres::PostgresStore;
use clio_types::{AttributeSchema, PersonaPreference, UpdateRule};
use std::sync::Arc;

use crate::test_unique::unique;

fn open_pg() -> Option<PostgresStore> {
    let url = postgres_url_or_default(std::env::var("CLIO_DATABASE_URL"));
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    let store = match PostgresStore::open(&url, kms) {
        Ok(store) => store,
        Err(err) => {
            crate::pg_skip::unreachable_pg_or_skip("postgres continuous tests", &err);
            return None;
        }
    };
    Some(store)
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

/// Dual-backend parity — the same appendix fixture on Postgres matches SQLite.
#[test]
fn t09_parity_with_sqlite() {
    let _guard = lock_pg_test();
    let Some(pg) = open_pg() else {
        return;
    };
    let bank = unique("b");
    let mut sch = schema(&bank, "affinity", UpdateRule::Continuous);
    sch.alpha = Some(0.5);
    sch.trend_window = Some(3);
    pg.put_attribute_schema(&sch, "agent").unwrap();
    let got = pg
        .get_attribute_schema(&bank, "affinity")
        .unwrap()
        .expect("declared");
    assert_eq!(got.update_rule, UpdateRule::Continuous);
    let fixture: [(f64, f64, f64); 3] = [(1.0, 1.0, 1.0), (0.0, 0.5, 0.75), (1.0, 0.75, 0.75)];
    for (i, (x, exp_state, exp_trend)) in fixture.into_iter().enumerate() {
        let out = pg
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
    let series = pg.continuous_series(&bank, "affinity", None, 100).unwrap();
    assert_eq!(series.len(), 3);
    assert!((series[2].state - 0.75).abs() < 1e-9);
    // Same numbers as the SQLite run (see `attribute_tests`): parity by fixture.
    assert_eq!(series[1].as_of, "2024-01-02");
    // Mismatched declaration and cross-bank fail closed on Postgres too.
    let err = pg
        .put_attribute_schema(&schema(&bank, "affinity", UpdateRule::Discrete), "agent")
        .unwrap_err();
    assert_eq!(err.code, clio_types::ErrorCode::Forbidden);
    let err = pg
        .observe_continuous(&unique("other"), "affinity", 1.0, "t", "agent")
        .unwrap_err();
    assert_eq!(err.code, clio_types::ErrorCode::WrongUpdateRule);
    // Argument guards fail closed on Postgres too.
    assert_eq!(
        pg.observe_continuous("", "affinity", 1.0, "t", "agent")
            .unwrap_err()
            .code,
        clio_types::ErrorCode::InvalidArgument
    );
    assert_eq!(
        pg.observe_continuous(&bank, "affinity", 1.0, "", "agent")
            .unwrap_err()
            .code,
        clio_types::ErrorCode::InvalidArgument
    );
    assert_eq!(
        pg.continuous_series(&bank, "", None, 10).unwrap_err().code,
        clio_types::ErrorCode::InvalidArgument
    );
}

/// Parity: the Postgres series attaches the preference context only from
/// the point where it became effective, matching SQLite. The context is supplied
/// at the later observation, so an `as_of` read that excludes it must return the
/// earlier point with no context.
#[test]
fn t09_context_effective_from_introduction_parity() {
    let _guard = lock_pg_test();
    let Some(pg) = open_pg() else { return };
    let bank = unique("b");
    pg.put_attribute_schema(&schema(&bank, "affinity", UpdateRule::Continuous), "agent")
        .unwrap();
    pg.observe_continuous(&bank, "affinity", 0.2, "2026-01-01", "agent")
        .unwrap();
    pg.observe_continuous(&bank, "affinity", 0.8, "2026-02-01", "agent")
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
    pg.put_preference_state(&pref, "seeder").unwrap();

    let full = pg.continuous_series(&bank, "affinity", None, 100).unwrap();
    assert_eq!(full.len(), 2);
    assert!(
        full[0].context.is_none(),
        "pre-context point must not carry it: {full:?}"
    );
    assert_eq!(
        full[1].context.as_deref(),
        Some("kept short after feedback")
    );
    let early = pg
        .continuous_series(&bank, "affinity", Some("2026-01-15"), 100)
        .unwrap();
    assert_eq!(early.len(), 1);
    assert!(
        early[0].context.is_none(),
        "pre-context point-in-time read must not leak it: {early:?}"
    );
}

/// on Postgres: four independent clients observing the same key serialize
/// via the per-key advisory lock; every observation is retained exactly once.
#[test]
fn t14_pg_concurrent_observes_serialized() {
    let _guard = lock_pg_test();
    let Some(s1) = open_pg() else { return };
    let url = postgres_url_or_default(std::env::var("CLIO_DATABASE_URL"));
    let bank = unique("b");
    s1.put_attribute_schema(&schema(&bank, "affinity", UpdateRule::Continuous), "agent")
        .unwrap();
    let handles: Vec<_> = [0.0f64, 1.0, 2.0, 3.0]
        .iter()
        .map(|v| {
            let s = PostgresStore::open(&url, Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>)
                .unwrap();
            let v = *v;
            let bank = bank.clone();
            std::thread::spawn(move || {
                s.observe_continuous(&bank, "affinity", v, "2024-01-01", "agent")
            })
        })
        .collect();
    for h in handles {
        h.join().expect("observe thread").expect("observe ok");
    }
    let all = s1.continuous_series(&bank, "affinity", None, 100).unwrap();
    assert_eq!(all.len(), 4, "every observation applied exactly once");
    // The schema declares α=0.5; the retained states must form a valid EMA
    // chain over some serial permutation of the submitted values.
    let alpha = 0.5;
    let mut recovered: Vec<i64> = Vec::new();
    for (i, p) in all.iter().enumerate() {
        let x = if i == 0 {
            p.state
        } else {
            (p.state - (1.0 - alpha) * all[i - 1].state) / alpha
        };
        assert!(
            (x - x.round()).abs() < 1e-6 && (-1e-6..=3.0 + 1e-6).contains(&x),
            "reconstructed observation {x} is not one of the submitted values"
        );
        recovered.push(x.round() as i64);
    }
    recovered.sort_unstable();
    assert_eq!(recovered, vec![0, 1, 2, 3], "no lost updates");
}
