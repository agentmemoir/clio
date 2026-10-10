// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//! Tests for `belief_observe`: create-vs-append gating, telemetry, and taxonomy.

use crate::history::belief_history;
use crate::observe::{BeliefObserveArgs, BeliefObserveOutcome, BeliefVecSink, belief_observe};
use crate::signals::BeliefSignalSource;
use clio_admission::{AdmissionPolicy, RankingEnv};
use clio_store::{BeliefStore, DekProvider, LocalDevKms, SqliteStore, Store};
use clio_types::{
    Actor, AmError, Bank, EpisodicType, EpistemicKind, ErrorCode, OpContext, RetrieveHit,
    SourceType, UpdateRule,
};
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

#[derive(Debug, Default)]
pub(crate) struct EmptySignals;

impl BeliefSignalSource for EmptySignals {
    fn neighbor_texts(&self, _bank_id: &str, _proposition: &str) -> Result<Vec<String>, AmError> {
        Ok(vec![])
    }
}

fn store() -> SqliteStore {
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    SqliteStore::open_in_memory(kms).expect("sqlite")
}

/// theta 0 with zero weights ⇒ score 0.0 clears the threshold (create passes).
pub(crate) fn passing_env() -> RankingEnv {
    RankingEnv::default()
}

/// Impossible threshold ⇒ create admission rejects.
pub(crate) fn failing_env() -> RankingEnv {
    RankingEnv {
        theta_admit: 1.1,
        ..RankingEnv::default()
    }
}

pub(crate) struct Setup {
    pub(crate) store: SqliteStore,
    pub(crate) ctx: OpContext,
    pub(crate) sink: BeliefVecSink,
}

pub(crate) fn setup() -> Setup {
    Setup {
        store: store(),
        ctx: OpContext::new(Bank::new(unique("bank")), Actor::Agent),
        sink: BeliefVecSink::default(),
    }
}

pub(crate) fn args<'a>(
    setup: &'a Setup,
    policy: AdmissionPolicy<'a>,
    proposition: &'a str,
    confidence: f64,
    source_type: &'a str,
    as_of: Option<&'a str>,
) -> BeliefObserveArgs<'a> {
    BeliefObserveArgs {
        ctx: &setup.ctx,
        subject_id: "subject-1",
        proposition,
        confidence,
        evidence_ref: "turn_1",
        source_type,
        context: None,
        as_of,
        policy,
        sink: &setup.sink,
        timestamp: "2026-06-01T00:00:00Z",
    }
}

#[test]
fn t02_create_passes_admission_and_starts_history() {
    let setup = setup();
    let env = passing_env();
    let out = belief_observe(
        &setup.store,
        &EmptySignals,
        &args(
            &setup,
            AdmissionPolicy::new(&env),
            "user likes coffee",
            0.4,
            "agent_inferred",
            None,
        ),
    )
    .unwrap();
    let BeliefObserveOutcome::Created { belief, decision } = out else {
        panic!("expected create");
    };
    assert!(decision.pass);
    assert_eq!(belief.confidence_history.len(), 1);
    assert_eq!(belief.current_confidence(), Some(0.4));
    let stored = setup
        .store
        .get_belief(setup.ctx.bank.as_str(), &belief.id)
        .unwrap()
        .expect("stored");
    assert_eq!(stored.proposition, "user likes coffee");
    assert_eq!(stored.confidence_history.len(), 1);
    assert_eq!(setup.sink.events()[0].outcome, "created");
}

#[test]
#[allow(clippy::float_cmp)]
fn t03_append_same_proposition_keeps_prior_entry() {
    let setup = setup();
    let env = passing_env();
    let first = belief_observe(
        &setup.store,
        &EmptySignals,
        &args(
            &setup,
            AdmissionPolicy::new(&env),
            "user likes coffee",
            0.4,
            "agent_inferred",
            None,
        ),
    )
    .unwrap();
    let BeliefObserveOutcome::Created { belief, .. } = first else {
        panic!("create");
    };
    // Whitespace/case variant resolves to the same identity.
    let mut a2 = args(
        &setup,
        AdmissionPolicy::new(&env),
        "  USER  likes COFFEE ",
        0.65,
        "agent_inferred",
        Some("2026-07-14"),
    );
    a2.evidence_ref = "turn_2";
    let out = belief_observe(&setup.store, &EmptySignals, &a2).unwrap();
    let BeliefObserveOutcome::Appended {
        belief_id,
        entry_count,
        ..
    } = out
    else {
        panic!("append");
    };
    assert_eq!(entry_count, 2);
    assert_eq!(belief_id, belief.id);
    let hist = belief_history(&setup.store, setup.ctx.bank.as_str(), &belief.id, None).unwrap();
    assert_eq!(hist.entries.len(), 2);
    assert_eq!(hist.entries[0].confidence, 0.4);
    assert_eq!(hist.entries[0].source_type, SourceType::AgentInferred);
    assert_eq!(hist.entries[1].confidence, 0.65);
    assert_eq!(hist.proposition, "user likes coffee");
}

#[test]
fn t03b_append_succeeds_even_when_fresh_create_would_fail() {
    let setup = setup();
    let pass = passing_env();
    belief_observe(
        &setup.store,
        &EmptySignals,
        &args(
            &setup,
            AdmissionPolicy::new(&pass),
            "user is vegetarian",
            0.4,
            "agent_inferred",
            None,
        ),
    )
    .unwrap();
    // A fresh create under this policy would reject (theta 1.1), but append must not.
    let fail = failing_env();
    let mut a2 = args(
        &setup,
        AdmissionPolicy::new(&fail),
        "user is vegetarian",
        0.8,
        "user_stated",
        Some("2026-07-14"),
    );
    a2.evidence_ref = "turn_9";
    let out = belief_observe(&setup.store, &EmptySignals, &a2).unwrap();
    assert!(matches!(
        out,
        BeliefObserveOutcome::Appended { entry_count: 2, .. }
    ));
}

#[test]
fn t06_invalid_source_type_and_range_rejected() {
    let setup = setup();
    let env = passing_env();
    let err = belief_observe(
        &setup.store,
        &EmptySignals,
        &args(
            &setup,
            AdmissionPolicy::new(&env),
            "p",
            0.5,
            "opinion",
            None,
        ),
    )
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    let err = belief_observe(
        &setup.store,
        &EmptySignals,
        &args(
            &setup,
            AdmissionPolicy::new(&env),
            "p",
            1.5,
            "user_stated",
            None,
        ),
    )
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::OutOfRange);
    let err = belief_observe(
        &setup.store,
        &EmptySignals,
        &args(
            &setup,
            AdmissionPolicy::new(&env),
            "   ",
            0.5,
            "user_stated",
            None,
        ),
    )
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
}

#[test]
fn t08_high_confidence_user_stated_stays_a_belief() {
    let setup = setup();
    let env = passing_env();
    let out = belief_observe(
        &setup.store,
        &EmptySignals,
        &args(
            &setup,
            AdmissionPolicy::new(&env),
            "user is vegetarian",
            0.99,
            "user_stated",
            None,
        ),
    )
    .unwrap();
    let BeliefObserveOutcome::Created { belief, .. } = out else {
        panic!("create");
    };
    // No item/explicit-fact row is created for the belief identity.
    assert!(
        setup
            .store
            .get_memory_item(setup.ctx.bank.as_str(), &belief.id)
            .unwrap()
            .is_none()
    );
    let hit = RetrieveHit::from_belief(&belief);
    assert_eq!(hit.epistemic_kind, EpistemicKind::Belief);
    assert_eq!(hit.source_type, Some(SourceType::UserStated));
    assert!(hit.requires_confidence_check);
}

#[test]
fn t09_create_below_threshold_is_rejected_without_write() {
    let setup = setup();
    let fail = failing_env();
    let out = belief_observe(
        &setup.store,
        &EmptySignals,
        &args(
            &setup,
            AdmissionPolicy::new(&fail),
            "user likes tea",
            0.4,
            "agent_inferred",
            None,
        ),
    )
    .unwrap();
    let BeliefObserveOutcome::Rejected { decision } = out else {
        panic!("expected reject");
    };
    assert!(!decision.pass);
    assert!(decision.rejection_reason.is_some());
    let key = clio_types::proposition_key("user likes tea");
    assert!(
        setup
            .store
            .find_belief_by_proposition(setup.ctx.bank.as_str(), &key)
            .unwrap()
            .is_none()
    );
    let event = &setup.sink.events()[0];
    assert_eq!(event.outcome, "rejected");
    assert_eq!(event.entry_count, 0);
}

#[derive(Debug)]
struct FailingSignals {
    code: ErrorCode,
}

impl BeliefSignalSource for FailingSignals {
    fn neighbor_texts(&self, _bank_id: &str, _proposition: &str) -> Result<Vec<String>, AmError> {
        Err(AmError::new(self.code, "signal store down"))
    }
}

#[test]
fn unavailable_signal_backends_fail_closed() {
    let setup = setup();
    let env = passing_env();
    let mut a = args(
        &setup,
        AdmissionPolicy::new(&env),
        "p",
        0.5,
        "user_stated",
        None,
    );
    let err = belief_observe(
        &setup.store,
        &FailingSignals {
            code: ErrorCode::Internal,
        },
        &a,
    )
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::FactorUnavailable);
    a = args(
        &setup,
        AdmissionPolicy::new(&env),
        "p",
        0.5,
        "user_stated",
        None,
    );
    let err = belief_observe(
        &setup.store,
        &FailingSignals {
            code: ErrorCode::FactorUnavailable,
        },
        &a,
    )
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::FactorUnavailable);
}

#[test]
fn t09b_no_sixth_category_or_forged_episodic_type() {
    // Beliefs have no category/episodic-type params to forge, and parsing
    // "belief" as an episodic type or update rule is rejected.
    assert!(EpisodicType::parse("belief").is_err());
    assert!(EpisodicType::parse("opinion").is_err());
    assert!(UpdateRule::parse("belief").is_err());
    assert!(SourceType::parse("opinion").is_err());
    assert!(EpistemicKind::parse("opinion").is_err());
    assert_eq!(
        EpistemicKind::parse("belief").unwrap(),
        EpistemicKind::Belief
    );
}
