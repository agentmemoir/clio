// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for `belief_observe` source-`context` tests:
//! create-path persistence and read surfacing, key omission without context,
//! and fail-closed validation before any write.
//!
//! ## Owns
//! - Context-focused `belief_observe` assertions (t14, t15), the append
//!   context rule (omitted / byte-identical accepted, differing rejected),
//!   and fail-closed validation before any write
//!
//! ## Does not own
//! - General gating/append behavior tests (`observe_tests`)
//! - Store-level context parity (`clio-store` belief tests)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::observe::BeliefObserveOutcome;
use crate::observe::belief_observe;
use crate::observe::observe_tests::{EmptySignals, args, passing_env, setup};
use clio_admission::AdmissionPolicy;
use clio_store::BeliefStore;
use clio_types::ErrorCode;

#[test]
fn t14_context_create_surfaces_on_reads_and_is_omitted_when_absent() {
    let setup = setup();
    let env = passing_env();
    let policy = AdmissionPolicy::new(&env);
    let expected = "said at the cafe on Friday";
    let mut a = args(
        &setup,
        policy.clone(),
        "user prefers oat milk",
        0.4,
        "user_stated",
        None,
    );
    a.context = Some(expected);
    let out = belief_observe(&setup.store, &EmptySignals, &a).unwrap();
    let BeliefObserveOutcome::Created { belief, .. } = out else {
        panic!("expected create");
    };
    let bank = setup.ctx.bank.as_str();
    let want = Some(expected.to_owned());
    let got = setup
        .store
        .get_belief(bank, &belief.id)
        .unwrap()
        .expect("get");
    assert_eq!(got.context, want);
    let listed = setup.store.list_beliefs(bank).unwrap();
    assert!(
        listed
            .iter()
            .any(|b| b.id == belief.id && b.context == want)
    );
    let key = clio_types::proposition_key("user prefers oat milk");
    let found = setup
        .store
        .find_belief_by_proposition(bank, &key)
        .unwrap()
        .expect("found");
    assert_eq!(found.context, want);
    // A belief written without context omits the key from its serialized form.
    let plain = args(
        &setup,
        policy,
        "user owns a kayak",
        0.4,
        "user_stated",
        None,
    );
    let out = belief_observe(&setup.store, &EmptySignals, &plain).unwrap();
    let BeliefObserveOutcome::Created { belief: plain, .. } = out else {
        panic!("expected create");
    };
    let json = serde_json::to_value(&plain).unwrap();
    assert!(json.get("context").is_none(), "{json}");
}

#[test]
fn t15_invalid_context_is_rejected_before_any_write() {
    let setup = setup();
    let env = passing_env();
    let policy = AdmissionPolicy::new(&env);
    let oversized = "x".repeat(clio_types::CONTEXT_MAX_BYTES + 1);
    for (bad, code) in [
        ("", ErrorCode::InvalidArgument),
        (oversized.as_str(), ErrorCode::OutOfRange),
    ] {
        let mut a = args(
            &setup,
            policy.clone(),
            "user likes tea",
            0.4,
            "agent_inferred",
            None,
        );
        a.context = Some(bad);
        let err = belief_observe(&setup.store, &EmptySignals, &a).unwrap_err();
        assert_eq!(err.code, code);
        // Fail closed: validation runs before the identity lookup and write.
        let stored = setup.store.list_beliefs(setup.ctx.bank.as_str()).unwrap();
        assert!(stored.is_empty(), "{stored:?}");
    }
}

/// Reproduction of the append-rejection contract: a supplied context on
/// append must be byte-identical to the stored value; a differing value is a
/// usage error and nothing is written.
#[test]
fn append_with_differing_context_is_rejected_without_write() {
    let setup = setup();
    let env = passing_env();
    let policy = AdmissionPolicy::new(&env);
    let mut first = args(
        &setup,
        policy.clone(),
        "user likes oat",
        0.4,
        "user_stated",
        None,
    );
    first.context = Some("said at the cafe");
    let out = belief_observe(&setup.store, &EmptySignals, &first).unwrap();
    let BeliefObserveOutcome::Created { belief, .. } = out else {
        panic!("expected create");
    };
    let bank = setup.ctx.bank.as_str();
    let stored = |id: &str| {
        let got = setup.store.get_belief(bank, id).unwrap().expect("present");
        (got.context.clone(), got.confidence_history.len())
    };
    assert_eq!(stored(&belief.id), (Some("said at the cafe".to_owned()), 1));

    // Differing context on an existing belief: usage error, no write at all.
    let mut a2 = args(
        &setup,
        policy.clone(),
        "user likes oat",
        0.65,
        "user_stated",
        Some("2026-07-14"),
    );
    a2.evidence_ref = "turn_2";
    a2.context = Some("said at home instead");
    let err = belief_observe(&setup.store, &EmptySignals, &a2).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert!(
        !err.message.contains("said at home instead"),
        "the supplied context must not be echoed: {}",
        err.message
    );
    assert_eq!(
        stored(&belief.id),
        (Some("said at the cafe".to_owned()), 1),
        "the rejection must leave the stored context and trajectory untouched"
    );
    let event = &setup.sink.events()[1];
    assert_eq!(event.outcome, "rejected");
    assert_eq!(event.belief_id, belief.id);
    assert!(event.rejection_reason.is_some());

    // Same failure when the belief has no stored context: a supplied value is
    // a context change, not an append.
    let mut plain = args(
        &setup,
        policy,
        "user owns a kayak",
        0.4,
        "user_stated",
        None,
    );
    plain.context = None;
    let out = belief_observe(&setup.store, &EmptySignals, &plain).unwrap();
    let BeliefObserveOutcome::Created { belief: kayak, .. } = out else {
        panic!("expected create");
    };
    let env2 = passing_env();
    let mut a3 = args(
        &setup,
        AdmissionPolicy::new(&env2),
        "user owns a kayak",
        0.7,
        "user_stated",
        Some("2026-08-01"),
    );
    a3.evidence_ref = "turn_3";
    a3.context = Some("bought it used");
    let err = belief_observe(&setup.store, &EmptySignals, &a3).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert_eq!(stored(&kayak.id), (None, 1));
}

/// Race loser whose supplied context differs from the winner's stored value:
/// the identity rule fails the retry append closed instead of replacing
/// the sealed context, and the stored value stays untouched.
#[test]
fn create_race_loser_with_differing_context_is_rejected() {
    let setup = setup();
    let env = passing_env();
    let bank = setup.ctx.bank.as_str().to_owned();
    let id = crate::observe::observe_flow_tests::seeded_proposition(
        &setup.store,
        &bank,
        "subject-1",
        "user likes espresso",
        Some("cafe framing"),
    );
    let mut a = crate::observe::observe_flow_tests::observe_args(
        &setup.ctx,
        &setup.sink,
        &env,
        "user likes espresso",
        0.7,
        "user_stated",
    );
    a.context = Some("a different framing");
    let store = crate::observe::observe_flow_tests::StaleFindStore {
        inner: setup.store,
        hide: std::sync::atomic::AtomicUsize::new(1),
    };
    let err = belief_observe(&store, &EmptySignals, &a).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    let got = store.inner.get_belief(&bank, &id).unwrap().expect("belief");
    assert_eq!(got.context.as_deref(), Some("cafe framing"));
    assert_eq!(got.confidence_history.len(), 1);
}

#[test]
fn append_preserves_context_and_accepts_byte_identical() {
    let setup = setup();
    let env = passing_env();
    let policy = AdmissionPolicy::new(&env);
    let mut first = args(
        &setup,
        policy.clone(),
        "user likes oat",
        0.4,
        "user_stated",
        None,
    );
    first.context = Some("said at the cafe");
    let out = belief_observe(&setup.store, &EmptySignals, &first).unwrap();
    let BeliefObserveOutcome::Created { belief, .. } = out else {
        panic!("expected create");
    };
    let bank = setup.ctx.bank.as_str();
    let ctx = |id: &str| {
        setup
            .store
            .get_belief(bank, id)
            .unwrap()
            .expect("present")
            .context
    };
    // Append without a context: the stored context is preserved untouched.
    let mut a2 = args(
        &setup,
        policy.clone(),
        "user likes oat",
        0.65,
        "user_stated",
        Some("2026-07-14"),
    );
    a2.evidence_ref = "turn_2";
    a2.context = None;
    belief_observe(&setup.store, &EmptySignals, &a2).unwrap();
    assert_eq!(ctx(&belief.id).as_deref(), Some("said at the cafe"));
    // Byte-identical context on append is accepted and the stored value stays.
    let mut a3 = args(
        &setup,
        policy,
        "user likes oat",
        0.7,
        "user_stated",
        Some("2026-08-01"),
    );
    a3.evidence_ref = "turn_3";
    a3.context = Some("said at the cafe");
    let out = belief_observe(&setup.store, &EmptySignals, &a3).unwrap();
    let BeliefObserveOutcome::Appended { entry_count, .. } = out else {
        panic!("expected append");
    };
    assert_eq!(entry_count, 3);
    assert_eq!(ctx(&belief.id).as_deref(), Some("said at the cafe"));
    let outcomes: Vec<String> = setup
        .sink
        .events()
        .iter()
        .map(|e| e.outcome.clone())
        .collect();
    assert_eq!(outcomes, ["created", "appended", "appended"]);
}
