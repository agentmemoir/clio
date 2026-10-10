// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for tests of `persona_observe_preference` (the
//! thin continuous adapter).
//!
//! ## Owns
//! - EMA movement + trend through the shared engine
//! - discrete-declared key rejection pointing at `persona_put_stable`
//! - Score retention across observes, budget after observe, concurrency
//! - Optional `context` preserve/replace/fail-closed behavior
//!
//! ## Boundary
//!
//! Continuous-path behavior changes should normally update tests here.

use crate::observe::{
    PersonaObserveOutcome, PersonaObservePreferenceArgs, persona_observe_preference,
};
use crate::stable::stable_tests::{EmptySignals, Setup, setup};
use crate::{DEFAULT_PERSONA_TOKEN_BUDGET, PersonaGetArgs, PersonaVecSink, persona_get};
use clio_admission::{AdmissionPolicy, RankingEnv};
use clio_store::{ContinuousStore, SqliteStore};
use clio_types::{Actor, AttributeSchema, Bank, ErrorCode, OpContext, UpdateRule};
use clio_write::update::ContinuousVecSink;

fn observe_args<'a>(s: &'a Setup, key: &'a str, value: f64) -> PersonaObservePreferenceArgs<'a> {
    PersonaObservePreferenceArgs {
        store: &s.store,
        continuous: &s.store,
        ctx: &s.ctx,
        key,
        observed_value: value,
        confidence: Some(0.7),
        source: Some("inferred"),
        evidence_ref: Some("turn-2"),
        context: None,
        policy: AdmissionPolicy::new(&s.env),
        signal_source: &EmptySignals,
        admission_sink: &s.admit_sink,
        continuous_sink: &s.cont_sink,
        persona_sink: &s.persona_sink,
        budget_tokens: DEFAULT_PERSONA_TOKEN_BUDGET,
        timestamp: "2026-09-10",
    }
}

/// Two observes move the EMA toward the observations; trend defined; the
/// engine math matches the shared contract (α = default 0.3).
#[test]
fn t04_observe_moves_ema_and_trend() {
    let s = setup();
    let out = persona_observe_preference(&observe_args(&s, "brevity_affinity", 1.0)).unwrap();
    let PersonaObserveOutcome::Observed {
        state,
        trend,
        admission_score,
        dropped_keys,
    } = out
    else {
        panic!("expected observed");
    };
    assert!((state - 1.0).abs() < 1e-9);
    assert!((trend - 1.0).abs() < 1e-9);
    assert!(admission_score > 0.0);
    assert!(dropped_keys.is_empty(), "{dropped_keys:?}");
    let out = persona_observe_preference(&observe_args(&s, "brevity_affinity", 0.0)).unwrap();
    let PersonaObserveOutcome::Observed { state, trend, .. } = out else {
        panic!("expected observed");
    };
    // Shared engine: s2 = 0.3·0 + 0.7·1.0 = 0.7; trend = mean(1.0, 0.7).
    assert!((state - 0.7).abs() < 1e-9, "{state}");
    assert!((trend - 0.85).abs() < 1e-9, "{trend}");
    // One preference row reflects the engine state and the create-time score.
    let doc = (&s.store as &dyn clio_store::PersonaStore)
        .get_persona_document(s.ctx.bank.as_str())
        .unwrap();
    assert_eq!(doc.preferences.len(), 1);
    let p = &doc.preferences[0];
    assert!((p.value - 0.7).abs() < 1e-9);
    assert!((p.admission_score - admission_score).abs() < 1e-12);
    // Persona telemetry for both observes (path=continuous, key, actor, bank).
    let persona_events = s.persona_sink.events();
    assert_eq!(persona_events.len(), 2);
    assert_eq!(persona_events[0].operation, "persona_observe_preference");
    assert_eq!(persona_events[0].path, "continuous");
    assert_eq!(persona_events[0].key, "brevity_affinity");
    assert_eq!(persona_events[0].actor, "agent");
    assert!(persona_events[1].state.is_some());
    // The engine's trend reaches the telemetry sink (no discarded trend).
    assert_eq!(persona_events[1].trend, Some(0.85));
    assert!(persona_events[0].trend.is_some());
}

/// A discrete-declared key is rejected with a pointer to the stable path;
/// categorical strings cannot arrive (typed f64 surface).
#[test]
fn t05_discrete_declared_key_rejected() {
    let s = setup();
    let schema = AttributeSchema {
        bank_id: s.ctx.bank.as_str().into(),
        attr_key: "style_label".into(),
        update_rule: UpdateRule::Discrete,
        alpha: None,
        trend_window: None,
        bounds_lo: None,
        bounds_hi: None,
        clamp_values: false,
        created_at: "t".into(),
        updated_at: None,
    };
    (&s.store as &dyn clio_store::ContinuousStore)
        .put_attribute_schema(&schema, "agent")
        .unwrap();
    let err = persona_observe_preference(&observe_args(&s, "style_label", 0.5)).unwrap_err();
    assert_eq!(err.code, ErrorCode::WrongUpdateRule);
    assert!(
        err.message.contains("persona_put_stable"),
        "{}",
        err.message
    );
    let events = s.persona_sink.events();
    assert!(events.is_empty(), "{events:?}");
    // Structural rejects: empty key, non-finite value, bad confidence, empty bank.
    assert_eq!(
        persona_observe_preference(&observe_args(&s, "", 0.5))
            .unwrap_err()
            .code,
        ErrorCode::InvalidArgument
    );
    assert_eq!(
        persona_observe_preference(&observe_args(&s, "k", f64::NAN))
            .unwrap_err()
            .code,
        ErrorCode::OutOfRange
    );
    let mut args = observe_args(&s, "k", 0.5);
    args.confidence = Some(2.0);
    assert_eq!(
        persona_observe_preference(&args).unwrap_err().code,
        ErrorCode::OutOfRange
    );
    let other = OpContext::new(Bank::new(""), Actor::Agent);
    let mut args = observe_args(&s, "k", 0.5);
    args.ctx = &other;
    assert_eq!(
        persona_observe_preference(&args).unwrap_err().code,
        ErrorCode::MissingBank
    );
}

/// Create admission is scored once; later observes retain the create-time score.
#[test]
fn create_admission_stamps_score_and_later_observes_retain_it() {
    let s = setup();
    let first = persona_observe_preference(&observe_args(&s, "pref_key", 0.4)).unwrap();
    let PersonaObserveOutcome::Observed {
        admission_score, ..
    } = first
    else {
        panic!("expected observed");
    };
    let second = persona_observe_preference(&observe_args(&s, "pref_key", 0.6)).unwrap();
    let PersonaObserveOutcome::Observed {
        admission_score: again,
        ..
    } = second
    else {
        panic!("expected observed");
    };
    assert!((admission_score - again).abs() < 1e-12);
    // The reject branch: impossible threshold on a fresh key.
    let failing = RankingEnv {
        theta_admit: 1.1,
        ..RankingEnv::default()
    };
    let mut args = observe_args(&s, "rejected_key", 0.4);
    args.policy = AdmissionPolicy::new(&failing);
    let out = persona_observe_preference(&args).unwrap();
    assert!(matches!(out, PersonaObserveOutcome::Rejected { .. }));
    let doc = (&s.store as &dyn clio_store::PersonaStore)
        .get_persona_document(s.ctx.bank.as_str())
        .unwrap();
    assert!(doc.preferences.iter().all(|p| p.key != "rejected_key"));
    assert!(
        s.persona_sink
            .events()
            .iter()
            .any(|e| e.outcome == "rejected" && e.key == "rejected_key")
    );
    // A rejected create writes nothing, including the attribute declaration
    // The key is not locked into continuous and stays writable by
    // persona_put_stable.
    assert!(
        (&s.store as &dyn clio_store::ContinuousStore)
            .get_attribute_schema(s.ctx.bank.as_str(), "rejected_key")
            .unwrap()
            .is_none()
    );
}

/// Concurrent observes on the same key serialize inside the engine's RMW;
/// every observation lands (no lost updates) and the final state is 1.0.
#[test]
fn t10_concurrent_observes_serialize() {
    let kms = std::sync::Arc::new(clio_store::LocalDevKms::new());
    let store = std::sync::Arc::new(SqliteStore::open_in_memory(kms).expect("sqlite"));
    let bank = format!(
        "bank-observe-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    );
    let ctx = OpContext::new(Bank::new(&bank), Actor::Agent);
    // First observe serializes the declaration on the shared store.
    let env = RankingEnv::default();
    let admit_sink = clio_admission::VecSink::default();
    let cont_sink = ContinuousVecSink::default();
    let persona_sink = PersonaVecSink::default();
    let first_args = PersonaObservePreferenceArgs {
        store: store.as_ref(),
        continuous: store.as_ref(),
        ctx: &ctx,
        key: "conc_pref",
        observed_value: 1.0,
        confidence: None,
        source: Some("inferred"),
        evidence_ref: None,
        context: None,
        policy: AdmissionPolicy::new(&env),
        signal_source: &EmptySignals,
        admission_sink: &admit_sink,
        continuous_sink: &cont_sink,
        persona_sink: &persona_sink,
        budget_tokens: DEFAULT_PERSONA_TOKEN_BUDGET,
        timestamp: "2026-09-10",
    };
    persona_observe_preference(&first_args).unwrap();
    let total = 16;
    let per_thread = 4;
    let handles: Vec<_> = (0..total / per_thread)
        .map(|_| {
            let store = std::sync::Arc::clone(&store);
            let bank = bank.clone();
            std::thread::spawn(move || {
                let ctx = OpContext::new(Bank::new(&bank), Actor::Agent);
                let env = RankingEnv::default();
                let thread_admit_sink = clio_admission::VecSink::default();
                let thread_persona_sink = PersonaVecSink::default();
                let thread_cont_sink = ContinuousVecSink::default();
                for _ in 0..per_thread {
                    let args = PersonaObservePreferenceArgs {
                        store: store.as_ref(),
                        continuous: store.as_ref(),
                        ctx: &ctx,
                        key: "conc_pref",
                        observed_value: 1.0,
                        confidence: None,
                        source: Some("inferred"),
                        evidence_ref: None,
                        context: None,
                        policy: AdmissionPolicy::new(&env),
                        signal_source: &EmptySignals,
                        admission_sink: &thread_admit_sink,
                        continuous_sink: &thread_cont_sink,
                        persona_sink: &thread_persona_sink,
                        budget_tokens: DEFAULT_PERSONA_TOKEN_BUDGET,
                        timestamp: "2026-09-10",
                    };
                    let out = persona_observe_preference(&args).unwrap();
                    assert!(matches!(out, PersonaObserveOutcome::Observed { .. }));
                }
            })
        })
        .collect();
    for handle in handles {
        handle.join().expect("observe thread");
    }
    let series = store
        .continuous_series(&bank, "conc_pref", None, total + 2)
        .unwrap();
    assert_eq!(series.len(), total + 1, "no observation may be lost");
    let last = series.last().expect("series");
    assert!((last.state - 1.0).abs() < 1e-9);
}

/// Budget enforcement after observe: handler truncation matches `persona_get`.
#[test]
fn budget_truncates_after_observe() {
    let s = setup();
    persona_observe_preference(&observe_args(&s, "cheap", 0.5)).unwrap();
    persona_observe_preference(&observe_args(&s, "lowly", 0.2)).unwrap();
    let out = persona_observe_preference(&observe_args(&s, "third", 0.8)).unwrap();
    let PersonaObserveOutcome::Observed { dropped_keys, .. } = out else {
        panic!("expected observed");
    };
    let get = persona_get(&PersonaGetArgs {
        store: &s.store,
        ctx: &s.ctx,
        budget_tokens: DEFAULT_PERSONA_TOKEN_BUDGET,
        timestamp: "t",
    })
    .unwrap();
    assert_eq!(dropped_keys, get.dropped_keys);
    // Zero budget drops everything; the document never grows unboundedly.
    let get = persona_get(&PersonaGetArgs {
        store: &s.store,
        ctx: &s.ctx,
        budget_tokens: 0,
        timestamp: "t",
    })
    .unwrap();
    assert_eq!(get.dropped_keys.len(), 3);
}

/// Optional `context` on the preference path: surfaced on create, preserved
/// when a later observation omits it, replaced when a new one is supplied, and
/// rejected (writing nothing) when invalid.
#[test]
fn context_roundtrips_on_preference_path() {
    let s = setup();
    let store = &s.store as &dyn clio_store::PersonaStore;
    let read_pref = |s: &Setup| {
        store
            .get_preference(s.ctx.bank.as_str(), "brevity_affinity")
            .unwrap()
            .expect("preference row")
    };
    // Create with context: surfaced on the preference and in the document.
    let mut args = observe_args(&s, "brevity_affinity", 1.0);
    args.context = Some("observed short replies all week");
    persona_observe_preference(&args).unwrap();
    assert_eq!(
        read_pref(&s).context.as_deref(),
        Some("observed short replies all week")
    );

    // Later observation without context preserves the stored context.
    persona_observe_preference(&observe_args(&s, "brevity_affinity", 0.5)).unwrap();
    assert_eq!(
        read_pref(&s).context.as_deref(),
        Some("observed short replies all week")
    );

    // A new context replaces the stored one.
    let mut args = observe_args(&s, "brevity_affinity", 0.8);
    args.context = Some("updated after week two");
    persona_observe_preference(&args).unwrap();
    assert_eq!(
        read_pref(&s).context.as_deref(),
        Some("updated after week two")
    );

    // Invalid contexts fail closed: no write, no attribute declaration.
    let oversized = "a".repeat(4097);
    for bad in ["", oversized.as_str()] {
        let mut args = observe_args(&s, "bad_pref", 0.4);
        args.context = Some(bad);
        let err = persona_observe_preference(&args).unwrap_err();
        assert!(
            err.code == ErrorCode::InvalidArgument || err.code == ErrorCode::OutOfRange,
            "{err}"
        );
    }
    let doc = store.get_persona_document(s.ctx.bank.as_str()).unwrap();
    assert!(!doc.preferences.iter().any(|p| p.key == "bad_pref"));
    assert!(
        (&s.store as &dyn clio_store::ContinuousStore)
            .get_attribute_schema(s.ctx.bank.as_str(), "bad_pref")
            .unwrap()
            .is_none(),
        "an invalid context must write nothing, including the declaration"
    );
}
