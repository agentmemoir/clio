// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module owns leaf-first ingest and write-latency acceptance tests.
//!
//! ## Owns
//! - Leaf readability, timing separation, and non-blocking maintenance assertions
//! - Ingest consolidation, admission, and cross-bank regression scenarios
//!
//! ## Does not own
//! - Production ingest or structural MemTree refresh behavior
//!
//! ## Boundary
//!
//! Changes to leaf-first ingest acceptance should normally require changes here.
//! Changes to production ingest orchestration should not require changes here.
//!
//! Keep this module focused on leaf publication and non-blocking maintenance acceptance.

use crate::chunk::Turn;
use crate::extract::{
    ExtractCandidate, ExtractRequest, Extractor, FixtureExtractor, PoisonExtractor,
};
use crate::ingest::{ParallelIngestArgs, WriteIngestConfig, parallel_ingest};
use crate::maintenance::{
    BufferMaintenance, LeafIndex, MaintenanceHook, MaintenanceSignal, NoopMaintenance,
    SharedMaintenanceHook,
};
use crate::schema::default_event_fields;
use crate::telemetry::VerifyVecSink;
use crate::verify::VerifyConfig;
use clio_admission::decision::Candidate;
use clio_admission::gated::SignalSource;
use clio_admission::{AdmissionPolicy, RankingEnv, VecSink};
use clio_types::{
    Actor, AmError, Bank, EpisodicType, EpistemicKind, ErrorCode, ItemKind, MemoryItem, OpContext,
};
use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// Maintenance hook that blocks inside `on_leaf_attached` until released, so a
/// test can prove the write path did not wait on it.
#[derive(Debug)]
struct BlockingMaintenance {
    entered: AtomicBool,
    released: Mutex<bool>,
    cv: Condvar,
    inner: BufferMaintenance,
}

impl BlockingMaintenance {
    fn new() -> Self {
        Self {
            entered: AtomicBool::new(false),
            released: Mutex::new(false),
            cv: Condvar::new(),
            inner: BufferMaintenance::default(),
        }
    }

    fn entered(&self) -> bool {
        self.entered.load(Ordering::SeqCst)
    }

    fn release(&self) {
        let mut released = self.released.lock().expect("release lock");
        *released = true;
        self.cv.notify_all();
    }

    fn signals(&self) -> Vec<MaintenanceSignal> {
        self.inner.signals()
    }
}

impl MaintenanceHook for BlockingMaintenance {
    fn on_leaf_attached(&self, signal: MaintenanceSignal) {
        self.entered.store(true, Ordering::SeqCst);
        let mut released = self.released.lock().expect("release lock");
        while !*released {
            released = self.cv.wait(released).expect("release wait");
        }
        self.inner.on_leaf_attached(signal);
    }
}

fn wait_until_entered(hook: &BlockingMaintenance) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !hook.entered() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    hook.entered()
}

#[derive(Debug, Default)]
struct EmptyNeighbors;

impl SignalSource for EmptyNeighbors {
    fn neighbor_texts(&self, _item: &MemoryItem) -> Result<Vec<String>, AmError> {
        Ok(vec![])
    }
    fn prior_age_days(&self, _item: &MemoryItem) -> Result<Option<f64>, AmError> {
        Ok(None)
    }
}

fn base_item(bank: &str) -> MemoryItem {
    MemoryItem {
        id: "leaf".into(),
        bank_id: bank.into(),
        subject_id: "subj".into(),
        kind: ItemKind::Episodic,
        category: None,
        episodic_type: Some(EpisodicType::Temporal),
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        admission_score: None,
        source_ref: None,
        snapshot: None,
        gist: None,
        valid_from: "t0".into(),
        valid_until: None,
        tx_from: "t0".into(),
        tx_until: None,
        created_at: "t0".into(),
        updated_at: "t0".into(),
        context: None,
    }
}

fn run_ingest(
    extractor: &(dyn Extractor + Sync),
    turns: &[Turn],
    config: WriteIngestConfig,
    leaves: &LeafIndex,
    maintenance: SharedMaintenanceHook,
    write: &mut dyn FnMut(&MemoryItem) -> Result<(), AmError>,
) -> crate::ingest::IngestOutcome {
    let ctx = OpContext::new(Bank::new("bank-1"), Actor::Agent);
    let env = RankingEnv::system_defaults();
    let admit = VecSink::default();
    let verify_sink = VerifyVecSink::default();
    let fields = default_event_fields();
    parallel_ingest(
        &ParallelIngestArgs {
            extractor,
            turns,
            schema_template: json!({"entity": ""}),
            fields: &fields,
            verify: &VerifyConfig::default(),
            item_base: base_item("bank-1"),
            candidate: Candidate::Episodic(EpisodicType::Temporal),
            signal_source: &EmptyNeighbors,
            ctx: &ctx,
            policy: AdmissionPolicy::new(&env),
            admit_sink: &admit,
            verify_sink: &verify_sink,
            timestamp: "t0",
            config,
            leaves,
            maintenance,
        },
        write,
    )
    .expect("ingest")
}

#[test]
fn t05_t06_leaf_readable_before_slow_maintenance() {
    let mut fx = FixtureExtractor::new();
    fx.insert(
        "Ada",
        ExtractCandidate {
            snapshot: Some(json!({"entity": "Ada"})),
            gist: Some("g".into()),
            source_ref: "t".into(),
            diagnostic: None,
        },
    );
    let turns = vec![Turn::text("Ada")];
    let leaves = LeafIndex::new();
    let hook = Arc::new(BlockingMaintenance::new());
    // Watchdog: if dispatch ever regresses to synchronous, unblock after 5s so
    // the suite still terminates; the assertions below then fail instead.
    let watchdog = Arc::clone(&hook);
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(5));
        watchdog.release();
    });
    let response_started = Instant::now();
    let out = run_ingest(
        &fx,
        &turns,
        WriteIngestConfig::default(),
        &leaves,
        hook.clone(),
        &mut |_| Ok(()),
    );
    let response_elapsed = response_started.elapsed();
    let id = out.admitted[0].item_id.clone();
    // Ingest returned while the hook is still blocked on its dispatcher thread.
    assert!(wait_until_entered(&hook), "hook never entered");
    assert!(
        hook.signals().is_empty(),
        "hook completed on the write thread"
    );
    assert!(out.timing.time_to_queryable_ms <= response_elapsed.as_millis());
    assert_eq!(out.timing.structural_maintenance_ms, 0);
    assert!(leaves.get("bank-1", &id).is_some());
    hook.release();
    let deadline = Instant::now() + Duration::from_secs(5);
    while hook.signals().is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!hook.signals().is_empty(), "signal never delivered");
}

#[test]
fn t06b_nfr2_fixture_time_to_queryable_under_2s() {
    let mut fx = FixtureExtractor::new();
    fx.insert(
        "Ada",
        ExtractCandidate {
            snapshot: Some(json!({"entity": "Ada"})),
            gist: Some("g".into()),
            source_ref: "t".into(),
            diagnostic: None,
        },
    );
    let turns = vec![Turn::text("Ada")];
    let mut samples = Vec::new();
    for label in ["A", "B", "C", "reference"] {
        let leaves = LeafIndex::new();
        let out = run_ingest(
            &fx,
            &turns,
            WriteIngestConfig::for_profile(label),
            &leaves,
            Arc::new(NoopMaintenance),
            &mut |_| Ok(()),
        );
        assert_eq!(out.profile_label, label);
        // A non-empty `admitted` list proves the leaf-publish milestone ran, so
        // `time_to_queryable_ms` was assigned by the ingest path rather than
        // left at its zero default.
        assert!(!out.admitted.is_empty(), "profile {label} admitted nothing");
        samples.push(out.timing.time_to_queryable_ms);
        assert!(
            out.timing.time_to_queryable_ms < 2000,
            "profile {label} time_to_queryable_ms={}",
            out.timing.time_to_queryable_ms
        );
    }
    samples.sort_unstable();
    let p95_idx = samples.len().saturating_sub(1);
    assert!(samples[p95_idx] < 2000);
}

#[test]
fn t07_merged_snapshot_reverify() {
    // Two chunks with same entity → merge. The canonical snapshot is retained from
    // the first candidate; gated_store_verified re-runs span verify against the
    // merged source before commit, so the merged unit still admits.
    let mut fx = FixtureExtractor::new();
    fx.insert(
        "Ada one",
        ExtractCandidate {
            snapshot: Some(json!({"entity": "Ada"})),
            gist: Some("g".into()),
            source_ref: "1".into(),
            diagnostic: None,
        },
    );
    fx.insert(
        "Ada two",
        ExtractCandidate {
            snapshot: Some(json!({"entity": "Ada"})),
            gist: Some("gg".into()),
            source_ref: "2".into(),
            diagnostic: None,
        },
    );
    let turns = vec![Turn::text("Ada one"), Turn::text("Ada two")];
    let leaves = LeafIndex::new();
    let out = run_ingest(
        &fx,
        &turns,
        WriteIngestConfig {
            max_turns_per_chunk: 1,
            ..WriteIngestConfig::default()
        },
        &leaves,
        Arc::new(NoopMaintenance),
        &mut |_| Ok(()),
    );
    assert_eq!(out.admitted.len(), 1);
}

#[test]
fn t10_empty_or_all_fail_no_admit() {
    let leaves = LeafIndex::new();
    let mut wrote = false;
    let out = run_ingest(
        &FixtureExtractor::new(),
        &[],
        WriteIngestConfig::default(),
        &leaves,
        Arc::new(NoopMaintenance),
        &mut |_| {
            wrote = true;
            Ok(())
        },
    );
    assert!(!wrote);
    assert!(out.admitted.is_empty(), "admitted: {:?}", out.admitted);

    let poison = PoisonExtractor::new("p");
    let out2 = run_ingest(
        &poison,
        &[Turn::text("Ada")],
        WriteIngestConfig::default(),
        &leaves,
        Arc::new(NoopMaintenance),
        &mut |_| {
            wrote = true;
            Ok(())
        },
    );
    assert!(!wrote);
    assert!(out2.admitted.is_empty(), "admitted: {:?}", out2.admitted);
    assert!(!out2.retained.is_empty(), "{:?}", out2.retained);
}

#[test]
fn embedding_merge_flag_rejected() {
    let leaves = LeafIndex::new();
    let ctx = OpContext::new(Bank::new("bank-1"), Actor::Agent);
    let env = RankingEnv::system_defaults();
    let admit = VecSink::default();
    let verify_sink = VerifyVecSink::default();
    let fields = default_event_fields();
    let err = parallel_ingest(
        &ParallelIngestArgs {
            extractor: &FixtureExtractor::new(),
            turns: &[Turn::text("Ada")],
            schema_template: json!({}),
            fields: &fields,
            verify: &VerifyConfig::default(),
            item_base: base_item("bank-1"),
            candidate: Candidate::Episodic(EpisodicType::Temporal),
            signal_source: &EmptyNeighbors,
            ctx: &ctx,
            policy: AdmissionPolicy::new(&env),
            admit_sink: &admit,
            verify_sink: &verify_sink,
            timestamp: "t0",
            config: WriteIngestConfig {
                embedding_merge: true,
                ..WriteIngestConfig::default()
            },
            leaves: &leaves,
            maintenance: Arc::new(NoopMaintenance),
        },
        &mut |_| Ok(()),
    )
    .expect_err("flag");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
}

#[test]
fn all_or_nothing_skips_admit_on_partial_fail() {
    #[derive(Debug)]
    struct Mixed {
        calls: Mutex<u32>,
    }
    impl Extractor for Mixed {
        fn extract(&self, req: &ExtractRequest) -> Result<ExtractCandidate, AmError> {
            let mut n = self.calls.lock().unwrap();
            *n += 1;
            if req.source_text.contains("BAD") {
                Ok(ExtractCandidate {
                    snapshot: Some(json!({"entity": "WRONG"})),
                    gist: None,
                    source_ref: "b".into(),
                    diagnostic: None,
                })
            } else {
                Ok(ExtractCandidate {
                    snapshot: Some(json!({"entity": "Ada"})),
                    gist: None,
                    source_ref: "g".into(),
                    diagnostic: None,
                })
            }
        }
    }
    let leaves = LeafIndex::new();
    let mut wrote = false;
    let out = run_ingest(
        &Mixed {
            calls: Mutex::new(0),
        },
        &[Turn::text("Ada"), Turn::text("BAD")],
        WriteIngestConfig {
            max_turns_per_chunk: 1,
            all_or_nothing: true,
            ..WriteIngestConfig::default()
        },
        &leaves,
        Arc::new(NoopMaintenance),
        &mut |_| {
            wrote = true;
            Ok(())
        },
    );
    assert!(!wrote);
    assert!(out.admitted.is_empty(), "admitted: {:?}", out.admitted);
}
