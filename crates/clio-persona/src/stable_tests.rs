// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for tests of `persona_put_stable` and
//! `persona_get` (discrete path).
//!
//! ## Owns
//! - put/get roundtrip, invalidation + retained history
//! - scalar-intensity rejection pointing at `persona_observe_preference`
//! - Admission score stamping, telemetry, and cross-bank isolation
//! - Optional `context` roundtrip / omission / fail-closed behavior
//!
//! ## Boundary
//!
//! Discrete-path behavior changes should normally update tests here.

use crate::document::{PersonaGetArgs, persona_get};
use crate::stable::{PersonaPutStableArgs, PersonaPutStableOutcome, persona_put_stable};
use crate::{DEFAULT_PERSONA_TOKEN_BUDGET, PersonaVecSink};
use clio_admission::gated::SignalSource;
use clio_admission::{AdmissionPolicy, RankingEnv, VecSink};
use clio_store::SqliteStore;
use clio_types::{Actor, AmError, Bank, ErrorCode, OpContext};
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

/// Empty signal source: the tests use small unique banks, so no neighbors exist.
#[derive(Debug)]
pub(crate) struct EmptySignals;
impl SignalSource for EmptySignals {
    fn neighbor_texts(&self, _item: &clio_types::MemoryItem) -> Result<Vec<String>, AmError> {
        Ok(Vec::new())
    }
    fn prior_age_days(&self, _item: &clio_types::MemoryItem) -> Result<Option<f64>, AmError> {
        Ok(None)
    }
}

pub(crate) struct Setup {
    pub(crate) store: SqliteStore,
    pub(crate) ctx: OpContext,
    pub(crate) admit_sink: VecSink,
    pub(crate) persona_sink: PersonaVecSink,
    pub(crate) env: RankingEnv,
    pub(crate) cont_sink: clio_write::update::ContinuousVecSink,
}

pub(crate) fn setup() -> Setup {
    let kms = Arc::new(clio_store::LocalDevKms::new());
    let store = SqliteStore::open_in_memory(kms).expect("sqlite");
    let bank = unique("bank-stable");
    Setup {
        store,
        ctx: OpContext::new(Bank::new(&bank), Actor::Agent),
        admit_sink: VecSink::default(),
        persona_sink: PersonaVecSink::default(),
        env: RankingEnv::default(),
        cont_sink: clio_write::update::ContinuousVecSink::default(),
    }
}

pub(crate) fn put_args<'a>(s: &'a Setup, key: &'a str, value: &'a str) -> PersonaPutStableArgs<'a> {
    PersonaPutStableArgs {
        store: &s.store,
        continuous: &s.store,
        ctx: &s.ctx,
        key,
        value,
        confidence: Some(0.8),
        source: Some("explicit"),
        evidence_ref: Some("turn-1"),
        context: None,
        policy: AdmissionPolicy::new(&s.env),
        signal_source: &EmptySignals,
        admission_sink: &s.admit_sink,
        persona_sink: &s.persona_sink,
        budget_tokens: DEFAULT_PERSONA_TOKEN_BUDGET,
        timestamp: "2026-09-10",
        dry_run: false,
    }
}

/// + AC score stamp: put then `persona_get` shows the entry with its score.
#[test]
fn t01_put_then_get_returns_stable_with_score() {
    let s = setup();
    let out = persona_put_stable(&put_args(&s, "communication_style", "concise")).unwrap();
    let PersonaPutStableOutcome::Admitted {
        entry,
        decision,
        dropped_keys,
    } = out
    else {
        panic!("expected admission");
    };
    assert!(decision.pass);
    assert!((entry.admission_score - decision.admission_score).abs() < 1e-12);
    assert!(entry.admission_score > 0.0);
    assert!(dropped_keys.is_empty(), "{dropped_keys:?}");
    let doc = persona_get(&PersonaGetArgs {
        store: &s.store,
        ctx: &s.ctx,
        budget_tokens: DEFAULT_PERSONA_TOKEN_BUDGET,
        timestamp: "2026-09-10",
    })
    .unwrap()
    .document;
    assert_eq!(doc.stable.len(), 1);
    assert_eq!(doc.stable[0].key, "communication_style");
    assert_eq!(doc.stable[0].value, "concise");
    assert_eq!(doc.stable[0].source, "explicit");
    assert!((doc.stable[0].admission_score - decision.admission_score).abs() < 1e-12);
    // Admission + persona telemetry both emitted.
    assert!(
        s.admit_sink
            .events()
            .iter()
            .any(|e| e.operation == "admit_accept")
    );
    assert!(
        s.persona_sink
            .events()
            .iter()
            .any(|e| e.operation == "persona_put_stable"
                && e.path == "discrete"
                && e.outcome == "admitted"
                && e.actor == "agent"
                && e.bank == s.ctx.bank.as_str())
    );
}

/// Second put supersedes; latest visible; prior version in history.
#[test]
fn t02_second_put_invalidates_prior() {
    let s = setup();
    persona_put_stable(&put_args(&s, "communication_style", "concise")).unwrap();
    persona_put_stable(&put_args(&s, "communication_style", "detailed")).unwrap();
    let doc = persona_get(&PersonaGetArgs {
        store: &s.store,
        ctx: &s.ctx,
        budget_tokens: DEFAULT_PERSONA_TOKEN_BUDGET,
        timestamp: "2026-09-11",
    })
    .unwrap()
    .document;
    assert_eq!(doc.stable.len(), 1);
    assert_eq!(doc.stable[0].value, "detailed");
    let history = (&s.store as &dyn clio_store::PersonaStore)
        .stable_history(s.ctx.bank.as_str(), "communication_style")
        .unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].value, "concise");
    assert_eq!(history[0].valid_until.as_deref(), Some("2026-09-10"));
    assert!(history[1].valid_until.is_none());
}

/// scalar-intensity payload rejected, pointing at the EMA path.
#[test]
fn t03_numeric_intensity_rejected_on_stable_path() {
    let s = setup();
    let err = persona_put_stable(&put_args(&s, "brevity_affinity", "0.82")).unwrap_err();
    assert_eq!(err.code, ErrorCode::WrongUpdateRule);
    assert!(
        err.message.contains("persona_observe_preference"),
        "{}",
        err.message
    );
    assert!(s.persona_sink.events().is_empty(), "no write happened");
    // Same rejection for a continuous-declared key.
    let schema = clio_types::AttributeSchema {
        bank_id: s.ctx.bank.as_str().into(),
        attr_key: "declared_continuous".into(),
        update_rule: clio_types::UpdateRule::Continuous,
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
    let err = persona_put_stable(&put_args(&s, "declared_continuous", "warm")).unwrap_err();
    assert_eq!(err.code, ErrorCode::WrongUpdateRule);
    assert!(err.message.contains("persona_observe_preference"));
}

/// Admission reject: nothing written, structured outcome + telemetry.
#[test]
fn admission_reject_writes_nothing() {
    let s = setup();
    let failing = RankingEnv {
        theta_admit: 1.1,
        ..RankingEnv::default()
    };
    let mut args = put_args(&s, "style", "verbose");
    args.policy = AdmissionPolicy::new(&failing);
    let out = persona_put_stable(&args).unwrap();
    let PersonaPutStableOutcome::Rejected { decision } = out else {
        panic!("expected rejection");
    };
    assert!(!decision.pass);
    let doc = persona_get(&PersonaGetArgs {
        store: &s.store,
        ctx: &s.ctx,
        budget_tokens: DEFAULT_PERSONA_TOKEN_BUDGET,
        timestamp: "t",
    })
    .unwrap()
    .document;
    assert!(doc.stable.is_empty(), "{:?}", doc.stable);
    let events = s.persona_sink.events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].outcome, "rejected");
    assert!(
        s.admit_sink
            .events()
            .iter()
            .any(|e| e.operation == "admit_reject")
    );
}

/// Bank isolation — entries written in one bank are invisible elsewhere.
#[test]
fn t09_cross_bank_access_isolated() {
    let s = setup();
    persona_put_stable(&put_args(&s, "style", "concise")).unwrap();
    let other = OpContext::new(Bank::new(unique("bank-other")), Actor::Agent);
    let doc = persona_get(&PersonaGetArgs {
        store: &s.store,
        ctx: &other,
        budget_tokens: DEFAULT_PERSONA_TOKEN_BUDGET,
        timestamp: "t",
    })
    .unwrap()
    .document;
    assert!(
        doc.stable.is_empty() && doc.preferences.is_empty(),
        "another bank sees an empty document"
    );
    let mut args = put_args(&s, "style", "other-bank-value");
    args.ctx = &other;
    assert!(persona_put_stable(&args).is_ok());
    let doc = persona_get(&PersonaGetArgs {
        store: &s.store,
        ctx: &s.ctx,
        budget_tokens: DEFAULT_PERSONA_TOKEN_BUDGET,
        timestamp: "t",
    })
    .unwrap()
    .document;
    assert_eq!(doc.stable.len(), 1);
    assert_eq!(doc.stable[0].value, "concise");
}

/// Structural rejects: missing bank, empty key/value, bad confidence/source.
#[test]
fn structural_rejects() {
    let s = setup();
    let no_bank = OpContext::new(Bank::new(""), Actor::Agent);
    let mut args = put_args(&s, "k", "v");
    args.ctx = &no_bank;
    assert_eq!(
        persona_put_stable(&args).unwrap_err().code,
        ErrorCode::MissingBank
    );
    for (key, value) in [("", "v"), ("k", "")] {
        let args = put_args(&s, key, value);
        assert_eq!(
            persona_put_stable(&args).unwrap_err().code,
            ErrorCode::InvalidArgument
        );
    }
    let mut args = put_args(&s, "k", "v");
    args.confidence = Some(1.5);
    assert_eq!(
        persona_put_stable(&args).unwrap_err().code,
        ErrorCode::OutOfRange
    );
    let mut args = put_args(&s, "k", "v");
    args.source = Some("user_stated");
    assert_eq!(
        persona_put_stable(&args).unwrap_err().code,
        ErrorCode::InvalidArgument
    );
    let mut args = put_args(&s, "k", "v");
    args.timestamp = "";
    assert_eq!(
        persona_put_stable(&args).unwrap_err().code,
        ErrorCode::InvalidArgument
    );
    let rejected = s.persona_sink.events();
    assert!(rejected.is_empty(), "{rejected:?}");
}

/// Optional `context` on the stable path: sealed at rest, surfaced by reads,
/// omitted from JSON when unrecorded, and rejected (writing nothing) when
/// invalid.
#[test]
fn context_roundtrips_on_stable_path() {
    let s = setup();
    let store = &s.store as &dyn clio_store::PersonaStore;
    // With context: the document and the retained history surface it.
    let mut args = put_args(&s, "communication_style", "concise");
    args.context = Some("user said they prefer short replies");
    persona_put_stable(&args).unwrap();
    let doc = store.get_persona_document(s.ctx.bank.as_str()).unwrap();
    assert_eq!(
        doc.stable[0].context.as_deref(),
        Some("user said they prefer short replies")
    );
    let history = store
        .stable_history(s.ctx.bank.as_str(), "communication_style")
        .unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(
        history[0].context.as_deref(),
        Some("user said they prefer short replies")
    );

    // Without context: an ordinary update preserves the prior version's
    // context — the store carries it into the new version; the prior
    // version keeps its own context in history.
    persona_put_stable(&put_args(&s, "communication_style", "detailed")).unwrap();
    let doc = store.get_persona_document(s.ctx.bank.as_str()).unwrap();
    assert_eq!(doc.stable[0].value, "detailed");
    assert_eq!(
        doc.stable[0].context.as_deref(),
        Some("user said they prefer short replies")
    );
    let history = store
        .stable_history(s.ctx.bank.as_str(), "communication_style")
        .unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(
        history[0].context.as_deref(),
        Some("user said they prefer short replies")
    );
    assert_eq!(
        history[1].context.as_deref(),
        Some("user said they prefer short replies")
    );

    // A key never written with context has none and omits the JSON key.
    persona_put_stable(&put_args(&s, "plain_style", "neutral")).unwrap();
    let doc = store.get_persona_document(s.ctx.bank.as_str()).unwrap();
    let plain = doc.stable.iter().find(|e| e.key == "plain_style").unwrap();
    assert!(plain.context.is_none());
    assert!(
        !serde_json::to_string(plain)
            .unwrap()
            .contains("\"context\""),
        "records without context omit the key"
    );

    // Invalid contexts fail closed before any write (no telemetry, no row).
    let oversized = "a".repeat(4097);
    for bad in ["", oversized.as_str()] {
        let mut args = put_args(&s, "bad_key", "bad_value");
        args.context = Some(bad);
        let err = persona_put_stable(&args).unwrap_err();
        assert!(
            err.code == ErrorCode::InvalidArgument || err.code == ErrorCode::OutOfRange,
            "{err}"
        );
    }
    let doc = store.get_persona_document(s.ctx.bank.as_str()).unwrap();
    assert!(!doc.stable.iter().any(|e| e.key == "bad_key"));
    assert!(
        s.persona_sink.events().iter().all(|e| e.key != "bad_key"),
        "an invalid context must write nothing"
    );
}
