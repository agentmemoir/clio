// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for tests of the gated triple tools and telemetry.
//!
//! ## Owns
//! - Admission-gated `triple_add`, `triple_end`, `invalidate`, and event assertions
//! - Id-generation collision checks
//!
//! ## Does not own
//! - Store SQL behavior (see `clio-store` tests) or admission scoring
//!
//! ## Boundary
//!
//! Changes to triple tool wiring should normally update tests here.
//! Changes to store SQL or interval algebra should not require changes here.

use crate::triple::{
    TripleAddArgs, TripleVecSink, invalidate, triple_add, triple_end, triple_query, unique_id,
};
use clio_admission::{AdmissionPolicy, RankingEnv, SignalSource, VecSink};
use clio_store::{DekProvider, LocalDevKms, SqliteStore, Store, TimeAxis, TripleQuery};
use clio_types::{
    Actor, Bank, CONTEXT_MAX_BYTES, EpisodicType, EpistemicKind, ErrorCode, ItemKind, MemoryItem,
    OpContext, UpdateRule,
};
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Calls served so far in this process, so `unique` never repeats: the pid is
/// constant inside a process, so clock-plus-pid alone can return the same name
/// twice, and this name is a bank id other tests share.
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn unique(prefix: &str) -> String {
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("{prefix}-{n}-{}-{sequence}", std::process::id())
}

#[derive(Debug, Default)]
struct EmptySignals;

impl SignalSource for EmptySignals {
    fn neighbor_texts(&self, _item: &MemoryItem) -> Result<Vec<String>, clio_types::AmError> {
        Ok(vec![])
    }
}

fn store() -> SqliteStore {
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    SqliteStore::open_in_memory(kms).expect("sqlite")
}

#[test]
fn unique_ids_do_not_collide_on_shared_timestamp() {
    let ts = "2024-01-02T00:00:00Z";
    let e1 = unique_id("trp", ts);
    let e2 = unique_id("trp", ts);
    let i1 = unique_id("itm", ts);
    assert_ne!(e1, e2, "same-timestamp edge ids must differ");
    assert_ne!(e1, i1);
    assert!(e1.starts_with("trp-2024-01-02T00:00:00Z-"));
    assert!(i1.starts_with("itm-2024-01-02T00:00:00Z-"));
}

#[test]
fn t08_ungated_impossible_via_triple_add() {
    let s = store();
    let bank = unique("b");
    let ctx = OpContext::new(Bank::new(bank), Actor::Agent);
    let env = RankingEnv::default();
    let policy = AdmissionPolicy::new(&env);
    let admit = VecSink::default();
    let trip = TripleVecSink::default();
    let signals = EmptySignals;
    let r = triple_add(
        &s,
        &signals,
        &TripleAddArgs {
            ctx: &ctx,
            subject: "Alice",
            predicate: "lives_in",
            object: "Boston",
            epistemic_kind: EpistemicKind::Fact,
            source_type: None,
            confidence: Some(0.9),
            valid_from: Some("2024-01-01"),
            valid_until: None,
            supersede: true,
            snapshot_ref: None,
            gist_ref: None,
            context: None,
            update_rule: None,
            subject_id: "subj",
            policy,
            admit_sink: &admit,
            triple_sink: &trip,
            timestamp: "2024-01-02T00:00:00Z",
            dry_run: false,
        },
    )
    .unwrap();
    let (decision, outcome) = (r.decision, r.outcome);
    assert!(decision.pass);
    assert!(outcome.is_some());
    assert!(!admit.events().is_empty(), "no telemetry was emitted");
    assert_eq!(trip.events()[0].operation, "triple_add");
}

#[test]
#[allow(clippy::too_many_lines)]
fn t07_t12_continuous_and_telemetry() {
    let s = store();
    let bank = unique("b");
    let ctx = OpContext::new(Bank::new(bank.clone()), Actor::Agent);
    let env = RankingEnv::default();
    let policy = AdmissionPolicy::new(&env);
    let admit = VecSink::default();
    let trip = TripleVecSink::default();
    let err = triple_add(
        &s,
        &EmptySignals,
        &TripleAddArgs {
            ctx: &ctx,
            subject: "Alice",
            predicate: "lives_in",
            object: "0.82",
            epistemic_kind: EpistemicKind::Fact,
            source_type: None,
            confidence: None,
            valid_from: None,
            valid_until: None,
            supersede: true,
            snapshot_ref: None,
            gist_ref: None,
            context: None,
            update_rule: Some(UpdateRule::Continuous),
            subject_id: "subj",
            policy,
            admit_sink: &admit,
            triple_sink: &trip,
            timestamp: "2024-01-02T00:00:00Z",
            dry_run: false,
        },
    )
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::WrongUpdateRule);

    let policy = AdmissionPolicy::new(&env);
    let r = triple_add(
        &s,
        &EmptySignals,
        &TripleAddArgs {
            ctx: &ctx,
            subject: "Alice",
            predicate: "lives_in",
            object: "Boston",
            epistemic_kind: EpistemicKind::Fact,
            source_type: None,
            confidence: None,
            valid_from: Some("2024-01-01"),
            valid_until: None,
            supersede: true,
            snapshot_ref: None,
            gist_ref: None,
            context: None,
            update_rule: Some(UpdateRule::Discrete),
            subject_id: "subj",
            policy,
            admit_sink: &admit,
            triple_sink: &trip,
            timestamp: "2024-06-02T00:00:00Z",
            dry_run: false,
        },
    )
    .unwrap();
    let decision = r.decision;
    assert!(decision.pass);
    let end = triple_end(
        &s,
        &ctx,
        "Alice",
        "lives_in",
        None,
        "2025-01-01",
        "2025-01-01T00:00:00Z",
        None,
        &trip,
    )
    .unwrap();
    assert!(!end.closed_ids.is_empty(), "{:?}", end.closed_ids);
    assert!(trip.events().iter().any(|e| e.operation == "triple_end"));

    let item = MemoryItem {
        id: "sem1".into(),
        bank_id: bank.clone(),
        subject_id: "subj".into(),
        kind: ItemKind::Episodic,
        category: None,
        episodic_type: Some(EpisodicType::Gist),
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        admission_score: Some(0.8),
        source_ref: None,
        snapshot: Some(json!({"n": 1})),
        gist: Some("g".into()),
        valid_from: "t0".into(),
        valid_until: None,
        tx_from: "t0".into(),
        tx_until: None,
        created_at: "t0".into(),
        updated_at: "t0".into(),
        context: None,
    };
    s.create_memory_item(&item, "agent").unwrap();
    invalidate(
        &s,
        &ctx,
        "sem1",
        Some("sem2"),
        "2025-02-01T00:00:00Z",
        None,
        &trip,
    )
    .unwrap();
    assert!(trip.events().iter().any(|e| e.operation == "invalidate"));
    let rows = triple_query(
        &s,
        &TripleQuery {
            bank_id: bank,
            subject: None,
            predicate: None,
            object: None,
            as_of: None,
            time_axis: TimeAxis::Valid,
        },
    )
    .unwrap();
    assert!(rows.is_empty(), "still queryable: {rows:?}");
    assert_eq!(crate::triple::default_time_axis(), TimeAxis::Valid);
    let bad_ctx = OpContext::new(Bank::new(String::new()), Actor::Agent);
    let policy = AdmissionPolicy::new(&env);
    let err = triple_add(
        &s,
        &EmptySignals,
        &TripleAddArgs {
            ctx: &bad_ctx,
            subject: "Alice",
            predicate: "lives_in",
            object: "Boston",
            epistemic_kind: EpistemicKind::Fact,
            source_type: None,
            confidence: None,
            valid_from: None,
            valid_until: None,
            supersede: true,
            snapshot_ref: None,
            gist_ref: None,
            context: None,
            update_rule: None,
            subject_id: "subj",
            policy,
            admit_sink: &admit,
            triple_sink: &trip,
            timestamp: "2024-09-01T00:00:00Z",
            dry_run: false,
        },
    )
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::MissingBank);
}

#[test]
fn triple_add_surfaces_context_on_reads() {
    let s = store();
    let bank = unique("b");
    let ctx = OpContext::new(Bank::new(bank.clone()), Actor::Agent);
    let env = RankingEnv::default();
    let admit = VecSink::default();
    let trip = TripleVecSink::default();
    let r = triple_add(
        &s,
        &EmptySignals,
        &TripleAddArgs {
            ctx: &ctx,
            subject: "Alice",
            predicate: "lives_in",
            object: "Boston",
            epistemic_kind: EpistemicKind::Fact,
            source_type: None,
            confidence: Some(0.9),
            valid_from: Some("2024-01-01"),
            valid_until: None,
            supersede: true,
            snapshot_ref: None,
            gist_ref: None,
            context: Some("quarterly review minutes"),
            update_rule: None,
            subject_id: "subj",
            policy: AdmissionPolicy::new(&env),
            admit_sink: &admit,
            triple_sink: &trip,
            timestamp: "2024-01-02T00:00:00Z",
            dry_run: false,
        },
    )
    .unwrap();
    assert!(r.decision.pass);
    let edge_id = r.outcome.as_ref().expect("committed").new_edge_id.clone();
    let got = s.get_triple(&bank, &edge_id).unwrap().unwrap();
    assert_eq!(got.context.as_deref(), Some("quarterly review minutes"));
    let rows = triple_query(
        &s,
        &TripleQuery {
            bank_id: bank.clone(),
            subject: Some("Alice".into()),
            predicate: Some("lives_in".into()),
            object: None,
            as_of: None,
            time_axis: TimeAxis::Valid,
        },
    )
    .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].context.as_deref(), Some("quarterly review minutes"));
    let listed = s.list_triples(&bank).unwrap();
    assert_eq!(
        listed
            .iter()
            .find(|t| t.id == edge_id)
            .map(|t| t.context.clone()),
        Some(Some("quarterly review minutes".into()))
    );
}

#[test]
fn triple_add_without_context_omits_wire_key() {
    let s = store();
    let bank = unique("b");
    let ctx = OpContext::new(Bank::new(bank.clone()), Actor::Agent);
    let env = RankingEnv::default();
    let admit = VecSink::default();
    let trip = TripleVecSink::default();
    let r = triple_add(
        &s,
        &EmptySignals,
        &TripleAddArgs {
            ctx: &ctx,
            subject: "Alice",
            predicate: "lives_in",
            object: "Boston",
            epistemic_kind: EpistemicKind::Fact,
            source_type: None,
            confidence: Some(0.9),
            valid_from: Some("2024-01-01"),
            valid_until: None,
            supersede: true,
            snapshot_ref: None,
            gist_ref: None,
            context: None,
            update_rule: None,
            subject_id: "subj",
            policy: AdmissionPolicy::new(&env),
            admit_sink: &admit,
            triple_sink: &trip,
            timestamp: "2024-01-02T00:00:00Z",
            dry_run: false,
        },
    )
    .unwrap();
    let edge_id = r.outcome.as_ref().expect("committed").new_edge_id.clone();
    let got = s.get_triple(&bank, &edge_id).unwrap().unwrap();
    assert!(got.context.is_none());
    // No empty-string sentinel: the key is absent from the serialized record.
    let wire = serde_json::to_string(&got).unwrap();
    assert!(!wire.contains("context"), "{wire}");
}

#[test]
fn triple_add_rejects_bad_context_before_any_write() {
    let s = store();
    let bank = unique("b");
    let ctx = OpContext::new(Bank::new(bank.clone()), Actor::Agent);
    let env = RankingEnv::default();
    let admit = VecSink::default();
    let trip = TripleVecSink::default();
    let oversized = "a".repeat(CONTEXT_MAX_BYTES + 1);
    for (context, expected) in [
        ("", ErrorCode::InvalidArgument),
        (oversized.as_str(), ErrorCode::OutOfRange),
    ] {
        let err = triple_add(
            &s,
            &EmptySignals,
            &TripleAddArgs {
                ctx: &ctx,
                subject: "Alice",
                predicate: "lives_in",
                object: "Boston",
                epistemic_kind: EpistemicKind::Fact,
                source_type: None,
                confidence: Some(0.9),
                valid_from: Some("2024-01-01"),
                valid_until: None,
                supersede: true,
                snapshot_ref: None,
                gist_ref: None,
                context: Some(context),
                update_rule: None,
                subject_id: "subj",
                policy: AdmissionPolicy::new(&env),
                admit_sink: &admit,
                triple_sink: &trip,
                timestamp: "2024-01-02T00:00:00Z",
                dry_run: false,
            },
        )
        .unwrap_err();
        assert_eq!(err.code, expected, "context {context:?}");
    }
    // Nothing reached admission or the store: fail-closed before any write.
    assert!(admit.events().is_empty(), "admission ran anyway");
    assert!(trip.events().is_empty(), "a rejected call wrote a triple");
    let rows = triple_query(
        &s,
        &TripleQuery {
            bank_id: bank,
            subject: None,
            predicate: None,
            object: None,
            as_of: None,
            time_axis: TimeAxis::Valid,
        },
    )
    .unwrap();
    assert!(rows.is_empty(), "a row was left: {rows:?}");
}
