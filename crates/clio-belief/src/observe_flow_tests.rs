// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//! Telemetry, duplicate-evidence, missing-context, and create-race tests for
//! `belief_observe`.

use crate::history::belief_history;
use crate::observe::belief_observe;
use crate::observe::observe_tests::{EmptySignals, args, failing_env, passing_env, setup};
use crate::observe::{BeliefObserveArgs, BeliefObserveOutcome, BeliefVecSink};
use clio_admission::AdmissionPolicy;
use clio_store::{BeliefStore, SqliteStore};
use clio_types::{Actor, Bank, BeliefObject, ConfidenceEntry, ErrorCode, OpContext, SourceType};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

#[test]
fn t12_telemetry_emits_created_appended_and_rejected() {
    let setup = setup();
    let pass = passing_env();
    belief_observe(
        &setup.store,
        &EmptySignals,
        &args(
            &setup,
            AdmissionPolicy::new(&pass),
            "user likes coffee",
            0.4,
            "agent_inferred",
            None,
        ),
    )
    .unwrap();
    let mut a2 = args(
        &setup,
        AdmissionPolicy::new(&pass),
        "user likes coffee",
        0.5,
        "user_stated",
        Some("2026-07-01"),
    );
    a2.evidence_ref = "turn_2";
    belief_observe(&setup.store, &EmptySignals, &a2).unwrap();
    let fail = failing_env();
    belief_observe(
        &setup.store,
        &EmptySignals,
        &args(
            &setup,
            AdmissionPolicy::new(&fail),
            "unrelated claim",
            0.4,
            "agent_inferred",
            None,
        ),
    )
    .unwrap();
    let outcomes: Vec<String> = setup
        .sink
        .events()
        .iter()
        .map(|e| e.outcome.clone())
        .collect();
    assert_eq!(outcomes, ["created", "appended", "rejected"]);
    let created = &setup.sink.events()[0];
    assert!(created.admission_score.is_some());
    assert_eq!(created.source_type, "agent_inferred");
}

#[test]
fn duplicate_evidence_appends_not_deduplicates() {
    let setup = setup();
    let env = passing_env();
    belief_observe(
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
    // Same evidence_ref (and default as_of) twice ⇒ two appended entries.
    let scope = belief_history(
        &setup.store,
        setup.ctx.bank.as_str(),
        "user likes coffee",
        None,
    )
    .unwrap();
    let mut a2 = args(
        &setup,
        AdmissionPolicy::new(&env),
        "user likes coffee",
        0.4,
        "agent_inferred",
        Some("2026-06-02"),
    );
    a2.evidence_ref = "turn_1";
    belief_observe(&setup.store, &EmptySignals, &a2).unwrap();
    assert_eq!(scope.entries.len(), 1);
    let hist = belief_history(
        &setup.store,
        setup.ctx.bank.as_str(),
        "user likes coffee",
        None,
    )
    .unwrap();
    assert_eq!(hist.entries.len(), 2);
}

#[test]
fn missing_bank_and_subject_are_rejected() {
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
    let empty_ctx = OpContext::new(Bank::new(""), Actor::Agent);
    a.ctx = &empty_ctx;
    assert_eq!(
        belief_observe(&setup.store, &EmptySignals, &a)
            .unwrap_err()
            .code,
        ErrorCode::MissingBank
    );
    let mut a = args(
        &setup,
        AdmissionPolicy::new(&env),
        "p",
        0.5,
        "user_stated",
        None,
    );
    a.subject_id = "";
    assert_eq!(
        belief_observe(&setup.store, &EmptySignals, &a)
            .unwrap_err()
            .code,
        ErrorCode::InvalidArgument
    );
    let mut a = args(
        &setup,
        AdmissionPolicy::new(&env),
        "p",
        0.5,
        "user_stated",
        None,
    );
    a.timestamp = "";
    assert_eq!(
        belief_observe(&setup.store, &EmptySignals, &a)
            .unwrap_err()
            .code,
        ErrorCode::InvalidArgument
    );
}

/// Delegates to a real store but hides existing beliefs from proposition lookups
/// for `hide` calls, simulating the stale read a concurrent create-race loser sees.
pub(crate) struct StaleFindStore {
    pub(crate) inner: SqliteStore,
    pub(crate) hide: AtomicUsize,
}

impl BeliefStore for StaleFindStore {
    fn create_belief(&self, belief: &BeliefObject, actor: &str) -> Result<(), clio_types::AmError> {
        self.inner.create_belief(belief, actor)
    }

    fn append_confidence(
        &self,
        bank_id: &str,
        belief_id: &str,
        entry: &ConfidenceEntry,
        actor: &str,
    ) -> Result<(), clio_types::AmError> {
        self.inner
            .append_confidence(bank_id, belief_id, entry, actor)
    }

    fn append_confidence_with_context(
        &self,
        bank_id: &str,
        belief_id: &str,
        entry: &ConfidenceEntry,
        actor: &str,
        context: Option<&str>,
    ) -> Result<(), clio_types::AmError> {
        self.inner
            .append_confidence_with_context(bank_id, belief_id, entry, actor, context)
    }

    fn get_belief(
        &self,
        bank_id: &str,
        belief_id: &str,
    ) -> Result<Option<BeliefObject>, clio_types::AmError> {
        self.inner.get_belief(bank_id, belief_id)
    }

    /// `hide` is a budget: it hides exactly that many lookups and then
    /// delegates, which is what the second lookup on the race-loser path relies
    /// on. A call that finds the budget spent is behaving correctly.
    fn find_belief_by_proposition(
        &self,
        bank_id: &str,
        proposition_key: &str,
    ) -> Result<Option<BeliefObject>, clio_types::AmError> {
        let hid = self
            .hide
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |h| h.checked_sub(1))
            .is_ok();
        if hid {
            return Ok(None);
        }
        self.inner
            .find_belief_by_proposition(bank_id, proposition_key)
    }

    fn belief_history(
        &self,
        bank_id: &str,
        belief_id: &str,
        as_of: Option<&str>,
    ) -> Result<Vec<ConfidenceEntry>, clio_types::AmError> {
        self.inner.belief_history(bank_id, belief_id, as_of)
    }

    fn inspect_belief_ciphertext(
        &self,
        bank_id: &str,
        belief_id: &str,
    ) -> Result<Option<String>, clio_types::AmError> {
        self.inner.inspect_belief_ciphertext(bank_id, belief_id)
    }

    fn list_beliefs(&self, bank_id: &str) -> Result<Vec<BeliefObject>, clio_types::AmError> {
        self.inner.list_beliefs(bank_id)
    }
}

/// A belief created directly through the store (bypassing observe) so the
/// wrapper can simulate the race loser's stale `find` read. Returns the id.
pub(crate) fn seeded_proposition(
    store: &SqliteStore,
    bank: &str,
    subject: &str,
    proposition: &str,
    context: Option<&str>,
) -> String {
    let belief = BeliefObject {
        id: format!("bel-seeded-{}", unique_suffix()),
        bank_id: bank.into(),
        subject_id: subject.into(),
        proposition: proposition.into(),
        context: context.map(str::to_owned),
        // Store-derived at insert; never accepted from a caller.
        context_as_of: None,
        confidence_history: vec![
            ConfidenceEntry::new("2026-06-01", 0.4, "turn_0", SourceType::AgentInferred)
                .expect("entry"),
        ],
        created_at: "2026-06-01T00:00:00Z".into(),
        updated_at: "2026-06-01".into(),
    };
    let id = belief.id.clone();
    store.create_belief(&belief, "agent").expect("seed create");
    id
}

fn unique_suffix() -> String {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("{n}-{}", std::process::id())
}

/// Observe args that borrow only `ctx`/`sink` so the store field can be moved
/// into a wrapper first (disjoint field borrows).
pub(crate) fn observe_args<'a>(
    ctx: &'a OpContext,
    sink: &'a BeliefVecSink,
    env: &'a clio_admission::RankingEnv,
    proposition: &'a str,
    confidence: f64,
    source_type: &'a str,
) -> BeliefObserveArgs<'a> {
    BeliefObserveArgs {
        ctx,
        subject_id: "subject-1",
        proposition,
        confidence,
        evidence_ref: "turn_1",
        source_type,
        context: None,
        as_of: None,
        policy: AdmissionPolicy::new(env),
        sink,
        timestamp: "2026-06-01T00:00:00Z",
    }
}

/// Append preserves the stored context when omitted or byte-identical.
/// Race loser: stale read says the identity is missing, create hits the unique
/// index, and the retry path must serialize the loser as an append.
#[test]
fn create_race_loser_appends_instead_of_erroring() {
    let setup = setup();
    let env = passing_env();
    let bank = setup.ctx.bank.as_str().to_owned();
    seeded_proposition(
        &setup.store,
        &bank,
        "subject-1",
        "user likes espresso",
        None,
    );
    let store = StaleFindStore {
        inner: setup.store,
        hide: AtomicUsize::new(1),
    };
    let a = observe_args(
        &setup.ctx,
        &setup.sink,
        &env,
        "user likes espresso",
        0.7,
        "user_stated",
    );
    let out = belief_observe(&store, &EmptySignals, &a).unwrap();
    let BeliefObserveOutcome::Appended { entry_count, .. } = out else {
        panic!("race loser must append");
    };
    assert_eq!(entry_count, 2);
    let hist = belief_history(
        &store.inner,
        setup.ctx.bank.as_str(),
        "user likes espresso",
        None,
    )
    .unwrap();
    assert_eq!(hist.entries.len(), 2);
}

/// Same stale read, but the retry lookup is also stale (row vanished between
/// the failed create and the re-check): the original duplicate error propagates.
#[test]
fn create_race_loser_propagates_error_when_identity_is_gone() {
    let setup = setup();
    let env = passing_env();
    let bank = setup.ctx.bank.as_str().to_owned();
    seeded_proposition(&setup.store, &bank, "subject-1", "user likes rye", None);
    let store = StaleFindStore {
        inner: setup.store,
        hide: AtomicUsize::new(usize::MAX),
    };
    let err = belief_observe(
        &store,
        &EmptySignals,
        &observe_args(
            &setup.ctx,
            &setup.sink,
            &env,
            "user likes rye",
            0.5,
            "agent_inferred",
        ),
    )
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert!(err.message.contains("already exists"));
}

/// Two real threads observing one fresh proposition: exactly one create, the
/// other must append (never surface a duplicate-key error), history = 2 entries.
#[test]
fn concurrent_first_observes_serialize_as_append() {
    let setup = setup();
    let env = passing_env();
    let bank = setup.ctx.bank.as_str().to_owned();
    let store = Arc::new(setup.store);
    let outcomes: Vec<BeliefObserveOutcome> = std::thread::scope(|s| {
        let mut handles = Vec::new();
        for i in 0..2 {
            let store = Arc::clone(&store);
            let env = &env;
            let bank = bank.clone();
            handles.push(s.spawn(move || {
                let ctx = OpContext::new(Bank::new(bank), Actor::Agent);
                let sink = BeliefVecSink::default();
                let a = BeliefObserveArgs {
                    ctx: &ctx,
                    subject_id: "subject-1",
                    proposition: "user likes espresso",
                    confidence: 0.4 + f64::from(i) * 0.05,
                    evidence_ref: "turn_1",
                    source_type: "agent_inferred",
                    context: None,
                    as_of: None,
                    policy: AdmissionPolicy::new(env),
                    sink: &sink,
                    timestamp: "2026-06-01T00:00:00Z",
                };
                belief_observe(store.as_ref(), &EmptySignals, &a)
            }));
        }
        handles
            .into_iter()
            .map(|h| {
                h.join()
                    .expect("observe thread")
                    .expect("no error under race")
            })
            .collect()
    });
    let created = outcomes
        .iter()
        .filter(|o| matches!(o, BeliefObserveOutcome::Created { .. }))
        .count();
    let appended = outcomes
        .iter()
        .filter(|o| matches!(o, BeliefObserveOutcome::Appended { .. }))
        .count();
    assert_eq!(created, 1, "exactly one winner creates: {outcomes:?}");
    assert_eq!(appended, 1, "the loser appends: {outcomes:?}");
    let hist = belief_history(
        store.as_ref(),
        setup.ctx.bank.as_str(),
        "user likes espresso",
        None,
    )
    .unwrap();
    assert_eq!(hist.entries.len(), 2);
}
