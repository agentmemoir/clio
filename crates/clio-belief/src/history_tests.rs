// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//! Tests for `belief_history`: full trajectory, point-in-time, and not-found.

use crate::history::belief_history;
use crate::observe::{BeliefObserveArgs, belief_observe};
use crate::signals::BeliefSignalSource;
use clio_admission::{AdmissionPolicy, RankingEnv};
use clio_store::{DekProvider, LocalDevKms, SqliteStore};
use clio_types::{Actor, AmError, Bank, ErrorCode, OpContext, SourceType};
use std::sync::Arc;

#[derive(Debug, Default)]
struct EmptySignals;

impl BeliefSignalSource for EmptySignals {
    fn neighbor_texts(&self, _bank_id: &str, _proposition: &str) -> Result<Vec<String>, AmError> {
        Ok(vec![])
    }
}

#[allow(clippy::too_many_arguments)]
fn observe(
    store: &SqliteStore,
    ctx: &OpContext,
    env: &RankingEnv,
    proposition: &str,
    confidence: f64,
    source_type: &str,
    as_of: &str,
    evidence: &str,
) {
    let sink = crate::observe::BeliefVecSink::default();
    belief_observe(
        store,
        &EmptySignals,
        &BeliefObserveArgs {
            context: None,
            ctx,
            subject_id: "subject-1",
            proposition,
            confidence,
            evidence_ref: evidence,
            source_type,
            as_of: Some(as_of),
            policy: AdmissionPolicy::new(env),
            sink: &sink,
            timestamp: "2026-06-01T00:00:00Z",
        },
    )
    .unwrap();
}

fn fixture() -> (SqliteStore, OpContext) {
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    let store = SqliteStore::open_in_memory(kms).expect("sqlite");
    let ctx = OpContext::new(Bank::new("bank-history"), Actor::Agent);
    let env = RankingEnv::default();
    observe(
        &store,
        &ctx,
        &env,
        "user is vegetarian",
        0.4,
        "agent_inferred",
        "2026-06-01",
        "turn_1",
    );
    observe(
        &store,
        &ctx,
        &env,
        "user is vegetarian",
        0.65,
        "agent_inferred",
        "2026-07-14",
        "turn_2",
    );
    observe(
        &store,
        &ctx,
        &env,
        "user is vegetarian",
        0.9,
        "user_stated",
        "2026-09-02",
        "turn_3",
    );
    (store, ctx)
}

#[test]
fn t04_full_trajectory_by_id_and_proposition() {
    let (store, ctx) = fixture();
    let by_prop = belief_history(&store, ctx.bank.as_str(), "user is vegetarian", None).unwrap();
    assert_eq!(by_prop.entries.len(), 3);
    assert_eq!(by_prop.proposition, "user is vegetarian");
    assert_eq!(by_prop.current_confidence, Some(0.9));
    assert_eq!(by_prop.current_source_type, Some(SourceType::UserStated));
    let confidences: Vec<f64> = by_prop.entries.iter().map(|e| e.confidence).collect();
    assert_eq!(confidences, [0.4, 0.65, 0.9]);
    let source_types: Vec<SourceType> = by_prop.entries.iter().map(|e| e.source_type).collect();
    assert_eq!(
        source_types,
        [
            SourceType::AgentInferred,
            SourceType::AgentInferred,
            SourceType::UserStated
        ]
    );

    // Same result by exact id and by a differently-spaced proposition.
    let by_id = belief_history(&store, ctx.bank.as_str(), &by_prop.belief_id, None).unwrap();
    assert_eq!(by_id, by_prop);
    let spaced = belief_history(&store, ctx.bank.as_str(), "User   Is Vegetarian", None).unwrap();
    assert_eq!(spaced.belief_id, by_prop.belief_id);
    assert!(spaced.as_of.is_none());
}

#[test]
fn t05_as_of_returns_confidence_current_then() {
    let (store, ctx) = fixture();
    let mid = belief_history(
        &store,
        ctx.bank.as_str(),
        "user is vegetarian",
        Some("2026-08-01"),
    )
    .unwrap();
    assert_eq!(mid.entries.len(), 2);
    assert_eq!(mid.current_confidence, Some(0.65));
    assert_eq!(mid.as_of.as_deref(), Some("2026-08-01"));

    let before = belief_history(
        &store,
        ctx.bank.as_str(),
        "user is vegetarian",
        Some("2026-01-01"),
    )
    .unwrap();
    assert!(before.entries.is_empty(), "{:?}", before.entries);
    assert_eq!(before.current_confidence, None);

    let exact = belief_history(
        &store,
        ctx.bank.as_str(),
        "user is vegetarian",
        Some("2026-06-01"),
    )
    .unwrap();
    assert_eq!(exact.current_confidence, Some(0.4));
}

#[test]
fn unknown_belief_returns_structured_not_found() {
    let (store, ctx) = fixture();
    let err = belief_history(&store, ctx.bank.as_str(), "no such belief", None).unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
    assert!(err.message.contains("not found"));
    let err = belief_history(&store, "other-bank", "user is vegetarian", None).unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
}

#[test]
fn missing_bank_and_empty_target_are_rejected() {
    let (store, _ctx) = fixture();
    assert_eq!(
        belief_history(&store, "", "x", None).unwrap_err().code,
        ErrorCode::MissingBank
    );
    assert_eq!(
        belief_history(&store, "bank-history", "", None)
            .unwrap_err()
            .code,
        ErrorCode::InvalidArgument
    );
}
