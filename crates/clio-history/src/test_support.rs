// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the shared `clio-history` test fixtures: an
//! in-memory store, admission signal stubs, argument builders, and seeder
//! helpers.
//!
//! ## Owns
//! - `Setup`, `EMPTY_SIGNALS`, argument builders, and triple/preference/belief
//!   seed helpers
//!
//! ## Does not own
//! - Any production behavior or assertions (each test module owns those)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::{
    FailureRecordArgs, FailuresForTaskArgs, HistoryVecSink, PrepareRetryArgs, TaskGetArgs,
    TaskHistoryArgs, TaskUpsertArgs, TemporalHistoryArgs,
};
use clio_admission::{AdmissionPolicy, RankingEnv, SignalSource, VecSink};
use clio_store::{
    BeliefStore, ContinuousStore, DekProvider, IndexStore, LocalDevKms, PersonaStore, SqliteStore,
    Store,
};
use clio_types::{
    Actor, AmError, AttributeSchema, Bank, BeliefObject, ConfidenceEntry, EpistemicKind,
    HistoryPolicy, ItemKind, MemoryItem, OpContext, PersonaPreference, SourceType, TimeAxis,
    TripleRecord, UpdateRule,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Calls served so far in this process, so [`unique`] never repeats.
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Default event timestamp shared by the fixtures.
pub(crate) const T0: &str = "2026-06-01T00:00:00Z";
/// Later timestamp used for multi-version fixtures.
pub(crate) const T1: &str = "2026-06-02T00:00:00Z";
/// Third timestamp for >limit point-in-time fixtures.
pub(crate) const T2: &str = "2026-06-03T00:00:00Z";

/// A name distinct from every other call in this process.
///
/// The counter is the part that matters. The process id is constant inside one
/// process, so a name built from the clock and the pid alone repeats whenever
/// two calls land in the same clock tick — and here the name is a bank id in a
/// store that other tests share, so a repeat lets one test read another's rows.
pub(crate) fn unique(prefix: &str) -> String {
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("{prefix}-{nanos}-{}-{sequence}", std::process::id())
}

/// Gate signal stub with no neighbors and no prior (fully novel).
#[derive(Debug, Default)]
pub(crate) struct EmptySignals;

impl SignalSource for EmptySignals {
    fn neighbor_texts(&self, _item: &MemoryItem) -> Result<Vec<String>, AmError> {
        Ok(vec![])
    }

    fn prior_age_days(&self, _item: &MemoryItem) -> Result<Option<f64>, AmError> {
        Ok(None)
    }
}

pub(crate) static EMPTY_SIGNALS: EmptySignals = EmptySignals;

/// The default env clears admission (score 0 >= θ 0).
pub(crate) fn passing_env() -> RankingEnv {
    RankingEnv::default()
}

/// Impossible threshold ⇒ the gate rejects every candidate.
pub(crate) fn failing_env() -> RankingEnv {
    RankingEnv {
        theta_admit: 1.1,
        ..RankingEnv::default()
    }
}

pub(crate) struct Setup {
    pub(crate) store: Arc<SqliteStore>,
    pub(crate) ctx: OpContext,
    pub(crate) sink: HistoryVecSink,
    pub(crate) admission: VecSink,
}

pub(crate) fn setup() -> Setup {
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    let store = Arc::new(SqliteStore::open_in_memory(kms).expect("sqlite"));
    Setup {
        store,
        ctx: OpContext::new(Bank::new(unique("bank")), Actor::Agent),
        sink: HistoryVecSink::default(),
        admission: VecSink::default(),
    }
}

/// Build a hybrid retriever over the fixture store (no embedder/reranker).
pub(crate) fn retriever(setup: &Setup) -> clio_retrieve::HybridRetriever {
    clio_retrieve::HybridRetriever::new(
        Arc::clone(&setup.store) as Arc<dyn Store>,
        Arc::clone(&setup.store) as Arc<dyn IndexStore>,
    )
}

pub(crate) fn task_args<'a>(
    setup: &'a Setup,
    env: &'a RankingEnv,
    task_id: &'a str,
    definition: &'a str,
    steps: &'a [String],
    status: &'a str,
) -> TaskUpsertArgs<'a> {
    TaskUpsertArgs {
        context: None,
        store: setup.store.as_ref(),
        ctx: &setup.ctx,
        subject_id: "subject-1",
        task_id,
        definition,
        steps,
        status,
        evidence_ref: Some("turn_1"),
        policy: AdmissionPolicy::new(env),
        signal_source: &EMPTY_SIGNALS,
        admission_sink: &setup.admission,
        sink: &setup.sink,
        timestamp: T0,
        dry_run: false,
    }
}

pub(crate) fn get_args<'a>(setup: &'a Setup, task_id: &'a str) -> TaskGetArgs<'a> {
    TaskGetArgs {
        history: setup.store.as_ref(),
        store: setup.store.as_ref(),
        bank_id: setup.ctx.bank.as_str(),
        task_id,
    }
}

pub(crate) fn history_args<'a>(
    setup: &'a Setup,
    policy: &'a HistoryPolicy,
    task_id: &'a str,
    limit: Option<usize>,
) -> TaskHistoryArgs<'a> {
    TaskHistoryArgs {
        history: setup.store.as_ref(),
        bank_id: setup.ctx.bank.as_str(),
        task_id,
        limit,
        policy,
    }
}

pub(crate) fn failure_args<'a>(
    setup: &'a Setup,
    env: &'a RankingEnv,
    policy: &'a HistoryPolicy,
    task_id: &'a str,
    attempt_n: i64,
    what_failed: &'a str,
    lesson: &'a str,
) -> FailureRecordArgs<'a> {
    FailureRecordArgs {
        context: None,
        store: setup.store.as_ref(),
        ctx: &setup.ctx,
        subject_id: "subject-1",
        task_id,
        attempt_n,
        what_failed,
        lesson,
        evidence_ref: Some("trace_1"),
        policy: AdmissionPolicy::new(env),
        history_policy: policy,
        signal_source: &EMPTY_SIGNALS,
        admission_sink: &setup.admission,
        sink: &setup.sink,
        timestamp: T0,
        dry_run: false,
    }
}

pub(crate) fn query_args<'a>(
    setup: &'a Setup,
    policy: &'a HistoryPolicy,
    task_id_or_query: &'a str,
    limit: Option<usize>,
) -> FailuresForTaskArgs<'a> {
    FailuresForTaskArgs {
        history: setup.store.as_ref(),
        bank_id: setup.ctx.bank.as_str(),
        task_id_or_query,
        limit,
        policy,
        sink: &setup.sink,
    }
}

pub(crate) fn retry_args<'a>(
    setup: &'a Setup,
    policy: &'a HistoryPolicy,
    task_id: &'a str,
    limit: Option<usize>,
) -> PrepareRetryArgs<'a> {
    PrepareRetryArgs {
        history: setup.store.as_ref(),
        bank_id: setup.ctx.bank.as_str(),
        task_id,
        limit,
        policy,
        sink: &setup.sink,
    }
}

pub(crate) fn temporal_args<'a>(
    setup: &'a Setup,
    policy: &'a HistoryPolicy,
    target: &'a str,
    as_of: Option<&'a str>,
    time_axis: TimeAxis,
) -> TemporalHistoryArgs<'a> {
    TemporalHistoryArgs {
        history: setup.store.as_ref(),
        continuous: setup.store.as_ref(),
        beliefs: setup.store.as_ref(),
        bank_id: setup.ctx.bank.as_str(),
        target,
        as_of,
        time_axis,
        limit: None,
        policy,
        sink: &setup.sink,
    }
}

/// Declare a continuous attribute and apply observations.
pub(crate) fn seed_preference(setup: &Setup, key: &str, values: &[(f64, &str)]) {
    seed_preference_with_context(setup, key, values, None);
}

/// Same as [`seed_preference`], plus one `persona_preferences` row carrying
/// the given optional stored context for the same key.
pub(crate) fn seed_preference_with_context(
    setup: &Setup,
    key: &str,
    values: &[(f64, &str)],
    context: Option<&str>,
) {
    let bank = setup.ctx.bank.as_str();
    let schema = AttributeSchema {
        bank_id: bank.into(),
        attr_key: key.into(),
        update_rule: UpdateRule::Continuous,
        alpha: None,
        trend_window: None,
        bounds_lo: None,
        bounds_hi: None,
        clamp_values: false,
        created_at: T0.into(),
        updated_at: None,
    };
    setup
        .store
        .put_attribute_schema(&schema, "seeder")
        .expect("attribute schema");
    for (value, as_of) in values {
        setup
            .store
            .observe_continuous(bank, key, *value, as_of, "seeder")
            .expect("observe");
    }
    let last = values.last().copied().unwrap_or((0.0, T0));
    let pref = PersonaPreference {
        bank_id: bank.into(),
        key: key.into(),
        value: last.0,
        trend: last.0,
        confidence: None,
        source: "explicit".into(),
        source_ref: None,
        context: context.map(str::to_owned),
        admission_score: 0.9,
        as_of: last.1.into(),
    };
    setup
        .store
        .put_preference_state(&pref, "seeder")
        .expect("preference row");
}

/// Create a belief directly (bypassing admission) for read-path seeds.
pub(crate) fn seed_belief(setup: &Setup, proposition: &str, entries: &[(f64, &str)]) -> String {
    seed_belief_with_context(setup, proposition, entries, None)
}

/// Same as [`seed_belief`], with an optional stored belief context.
pub(crate) fn seed_belief_with_context(
    setup: &Setup,
    proposition: &str,
    entries: &[(f64, &str)],
    context: Option<&str>,
) -> String {
    let bank = setup.ctx.bank.as_str();
    let history: Vec<ConfidenceEntry> = entries
        .iter()
        .map(|(c, a)| {
            ConfidenceEntry::new(*a, *c, "evidence-1", SourceType::AgentInferred).expect("entry")
        })
        .collect();
    let belief = BeliefObject {
        context: context.map(str::to_owned),
        // Store-derived at insert never accepted from a caller.
        context_as_of: None,
        id: unique("seed-bel"),
        bank_id: bank.into(),
        subject_id: "subject-1".into(),
        proposition: proposition.into(),
        confidence_history: history,
        created_at: T0.into(),
        updated_at: T0.into(),
    };
    setup
        .store
        .create_belief(&belief, "seeder")
        .expect("belief");
    belief.id
}

/// Create one triple version directly for trajectory reads.
pub(crate) fn seed_triple(
    setup: &Setup,
    subject: &str,
    predicate: &str,
    object: &str,
    valid_from: &str,
    tx_from: &str,
) -> String {
    let bank = setup.ctx.bank.as_str();
    let item_id = unique("seed-triple-item");
    let item = MemoryItem {
        id: item_id,
        bank_id: bank.into(),
        subject_id: "subject-1".into(),
        kind: ItemKind::Episodic,
        category: None,
        episodic_type: Some(clio_types::EpisodicType::Triple),
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        admission_score: Some(0.9),
        source_ref: None,
        snapshot: None,
        gist: Some(format!("{subject} {predicate} {object}")),
        valid_from: valid_from.into(),
        valid_until: None,
        tx_from: tx_from.into(),
        tx_until: None,
        created_at: tx_from.into(),
        updated_at: tx_from.into(),
        context: None,
    };
    let edge = TripleRecord {
        context: None,
        id: unique("seed-edge"),
        bank_id: bank.into(),
        item_id: item.id.clone(),
        subject: subject.into(),
        predicate: predicate.into(),
        object: object.into(),
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        valid_from: valid_from.into(),
        valid_until: None,
        tx_from: tx_from.into(),
        tx_until: None,
        snapshot_ref: None,
        gist_ref: None,
        created_at: tx_from.into(),
        updated_at: None,
    };
    setup
        .store
        .commit_triple_add(&item, &edge, true, "seeder")
        .expect("seed triple");
    edge.id
}
