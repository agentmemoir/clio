// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the opt-in Postgres parity test for the
//! history tools it skips cleanly when not enabled.
//!
//! ## Owns
//! - Task/failure parity over Postgres with unique banks
//! - Temporal preference context surfacing parity over Postgres
//!
//! ## Does not own
//! - SQLite history behavior (`clio-store` history tests) or tool semantics
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::test_support::{EMPTY_SIGNALS, T0, T1, unique};
use crate::{
    FailureQueryOutcome, FailureRecordArgs, FailureRecordOutcome, TaskGetArgs, TaskHistoryArgs,
    TaskUpsertArgs, TaskUpsertOutcome, TemporalHistoryArgs, TemporalHistoryOutcome, failure_record,
    failures_for_task, task_get, task_history, task_upsert, temporal_history,
};
use clio_admission::{AdmissionPolicy, RankingEnv, VecSink};
use clio_store::{
    ContinuousStore, DekProvider, HistoryStore, LocalDevKms, PersonaStore, PgTestGuard,
    PostgresStore, Store, lock_pg_test,
};
use clio_types::{
    Actor, AttributeSchema, Bank, EpisodicType, EpistemicKind, HistoryPolicy, ItemKind, MemoryItem,
    OpContext, PersonaPreference, TimeAxis, TripleRecord, UpdateRule,
};
use std::sync::Arc;

fn open_pg() -> Option<(PgTestGuard, PostgresStore)> {
    if std::env::var("CLIO_PG_HISTORY_PARITY").as_deref() != Ok("1") {
        return None;
    }
    let url = std::env::var("CLIO_DATABASE_URL").ok()?;
    let guard = lock_pg_test();
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    PostgresStore::open(&url, kms)
        .ok()
        .map(|store| (guard, store))
}

#[test]
#[allow(clippy::too_many_lines)]
fn t14_postgres_task_and_failure_parity() {
    let Some((_guard, pg)) = open_pg() else {
        eprintln!(
            "skip postgres history parity: set CLIO_PG_HISTORY_PARITY=1 and CLIO_DATABASE_URL"
        );
        return;
    };
    let bank = unique("history-parity");
    let ctx = OpContext::new(Bank::new(bank.clone()), Actor::Agent);
    let env = RankingEnv::default();
    let policy = HistoryPolicy::default();
    let admission = VecSink::default();
    let sink = crate::HistoryVecSink::default();
    let steps = vec!["run tests".to_owned()];

    let upsert = |vars: &VecSink, ts: &'static str| {
        task_upsert(&TaskUpsertArgs {
            context: None,
            store: &pg,
            ctx: &ctx,
            subject_id: "subject-1",
            task_id: "task-a",
            definition: "fix flaky login",
            steps: &steps,
            status: "in_progress",
            evidence_ref: Some("turn_1"),
            policy: AdmissionPolicy::new(&env),
            signal_source: &EMPTY_SIGNALS,
            admission_sink: vars,
            sink: &sink,
            timestamp: ts,
            dry_run: false,
        })
        .expect("task upsert")
    };
    let TaskUpsertOutcome::Stored { record: first, .. } = upsert(&admission, T0) else {
        panic!("expected stored");
    };
    assert_eq!(first.version, 1);
    upsert(&admission, T1);
    let got = task_get(&TaskGetArgs {
        history: &pg,
        store: &pg,
        bank_id: &bank,
        task_id: "task-a",
    })
    .unwrap();
    assert_eq!(got.record.expect("record").definition, "fix flaky login");
    let hist = task_history(&TaskHistoryArgs {
        history: &pg,
        bank_id: &bank,
        task_id: "task-a",
        limit: Some(10),
        policy: &policy,
    })
    .unwrap();
    assert_eq!(hist.records.len(), 2);
    assert_eq!(hist.records[0].version, 2);
    let tasks = pg.list_tasks(&bank, 10).unwrap();
    assert!(
        tasks
            .iter()
            .any(|t| t.task_id == "task-a" && t.version == 2)
    );

    let record_failure = |attempt: i64, lesson: &'static str| {
        failure_record(&FailureRecordArgs {
            context: None,
            store: &pg,
            ctx: &ctx,
            subject_id: "subject-1",
            task_id: "task-a",
            attempt_n: attempt,
            what_failed: "assertion error",
            lesson,
            evidence_ref: Some("trace_1"),
            policy: AdmissionPolicy::new(&env),
            history_policy: &policy,
            signal_source: &EMPTY_SIGNALS,
            admission_sink: &admission,
            sink: &sink,
            timestamp: T0,
            dry_run: false,
        })
    };
    let FailureRecordOutcome::Stored {
        record: failure, ..
    } = record_failure(1, "read the fixture").expect("store failure")
    else {
        panic!("expected stored failure");
    };
    let read = pg
        .get_failure(&bank, &failure.id)
        .unwrap()
        .expect("failure readable");
    assert_eq!(read.lesson, "read the fixture");
    assert_eq!(read.epistemic_kind, EpistemicKind::Belief);
    let exact = pg.failures_for_task_exact(&bank, "task-a", 10).unwrap();
    assert_eq!(exact.len(), 1);
    assert_eq!(pg.list_failures(&bank, 10).unwrap().len(), 1);
    let duplicate = record_failure(1, "another lesson").expect_err("duplicate attempt");
    assert_eq!(duplicate.code, clio_types::ErrorCode::InvalidArgument);
    assert!(pg.get_failure(&bank, "missing").unwrap().is_none());
    assert!(pg.get_task(&bank, "missing").unwrap().is_none());

    let query = failures_for_task(&crate::FailuresForTaskArgs {
        history: &pg,
        bank_id: &bank,
        task_id_or_query: "task-a",
        limit: Some(5),
        policy: &policy,
        sink: &sink,
    })
    .unwrap();
    assert!(matches!(query, FailureQueryOutcome { ref mode, .. } if mode == "exact"));

    seed_pg_triple(&pg, &bank);
    let trajectory = pg.triple_history(&bank, "Alice", "lives_in", 10).unwrap();
    assert_eq!(trajectory.len(), 1);
}

#[test]
fn temporal_preference_context_postgres_parity() {
    // Parity: a preference observed on Postgres surfaces its stored
    // context through `temporal_history`, with as_of filtering unchanged.
    // Updated from the previously-leaking behavior: the context now rides only
    // the points at or after the write that introduced it.
    let Some((_guard, pg)) = open_pg() else {
        eprintln!(
            "skip postgres temporal context parity: \
             set CLIO_PG_HISTORY_PARITY=1 and CLIO_DATABASE_URL"
        );
        return;
    };
    let bank = unique("temporal-context-parity");
    let policy = HistoryPolicy::default();
    let sink = crate::HistoryVecSink::default();
    let schema = AttributeSchema {
        bank_id: bank.clone(),
        attr_key: "brevity_affinity".into(),
        update_rule: UpdateRule::Continuous,
        alpha: None,
        trend_window: None,
        bounds_lo: None,
        bounds_hi: None,
        clamp_values: false,
        created_at: T0.into(),
        updated_at: None,
    };
    pg.put_attribute_schema(&schema, "seeder").expect("schema");
    pg.observe_continuous(&bank, "brevity_affinity", 0.2, T0, "seeder")
        .expect("observe");
    pg.observe_continuous(&bank, "brevity_affinity", 0.8, T1, "seeder")
        .expect("observe");
    let pref = PersonaPreference {
        bank_id: bank.clone(),
        key: "brevity_affinity".into(),
        value: 0.8,
        trend: 0.8,
        confidence: None,
        source: "explicit".into(),
        source_ref: None,
        context: Some("kept short after feedback".into()),
        admission_score: 0.9,
        as_of: T1.into(),
    };
    pg.put_preference_state(&pref, "seeder")
        .expect("preference row");

    let read = |as_of: Option<&str>| {
        temporal_history(&TemporalHistoryArgs {
            history: &pg,
            continuous: &pg,
            beliefs: &pg,
            bank_id: &bank,
            target: "preference:brevity_affinity",
            as_of,
            time_axis: TimeAxis::Valid,
            limit: None,
            policy: &policy,
            sink: &sink,
        })
        .expect("temporal_history")
    };
    // The context was introduced at T1, so the earlier T0 point carries none and
    // the effective T1 point carries it.
    let TemporalHistoryOutcome::Preference { points, .. } = read(None) else {
        panic!("expected preference");
    };
    assert_eq!(points.len(), 2);
    assert!(
        points[0].context.is_none(),
        "the pre-context point must not carry it: {points:?}"
    );
    assert_eq!(
        points[1].context.as_deref(),
        Some("kept short after feedback")
    );
    // Regression: an as_of that excludes the T1 point must not leak the
    // context onto the surviving T0 point.
    let TemporalHistoryOutcome::Preference { points, .. } = read(Some(T0)) else {
        panic!("expected preference");
    };
    assert_eq!(points.len(), 1, "as_of filtering must still apply");
    assert!(
        points[0].context.is_none(),
        "a point-in-time read before the context existed must not leak it: {points:?}"
    );
    // A read at the context's effective time carries it on the T1 point.
    let TemporalHistoryOutcome::Preference { points, .. } = read(Some(T1)) else {
        panic!("expected preference");
    };
    assert_eq!(points.len(), 2);
    assert_eq!(
        points[1].context.as_deref(),
        Some("kept short after feedback")
    );
}

fn seed_pg_triple(pg: &PostgresStore, bank: &str) {
    let item_id = unique("pg-triple-item");
    let item = MemoryItem {
        id: item_id.clone(),
        bank_id: bank.into(),
        subject_id: "subject-1".into(),
        kind: ItemKind::Episodic,
        category: None,
        episodic_type: Some(EpisodicType::Triple),
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        admission_score: Some(0.9),
        source_ref: None,
        snapshot: None,
        gist: Some("Alice lives in Boston".into()),
        valid_from: T0.into(),
        valid_until: None,
        tx_from: T0.into(),
        tx_until: None,
        created_at: T0.into(),
        updated_at: T0.into(),
        context: None,
    };
    let edge = TripleRecord {
        context: None,
        id: unique("pg-edge"),
        bank_id: bank.into(),
        item_id,
        subject: "Alice".into(),
        predicate: "lives_in".into(),
        object: "Boston".into(),
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        valid_from: T0.into(),
        valid_until: None,
        tx_from: T0.into(),
        tx_until: None,
        snapshot_ref: None,
        gist_ref: None,
        created_at: T0.into(),
        updated_at: None,
    };
    pg.commit_triple_add(&item, &edge, true, "seeder").unwrap();
}
