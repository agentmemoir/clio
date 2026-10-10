// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the index outbox worker and its end-to-end search behavior over SQLite.
//!
//! ## Owns
//! - Leaf-write enqueue visibility, worker drain outcomes, backoff, and dim failures
//! - Bank isolation through the coordinator-driven pipeline
//!
//! ## Does not own
//! - Rebuild hooks and coverage recovery (see `rebuild_tests`)
//! - Raw transport or embed text assembly (`http_tests`/`embed_tests`)
//!
//! ## Boundary
//!
//! Changes to worker drain policy should normally update tests here.
//! Changes to backend search SQL should not require changes here.
//!
//! Keep this module focused on worker pipeline behavior.

use crate::embed::TeiEmbedder;
use crate::embed_test_support::{
    FailEmbedder, FakeEmbedServer, FakeEmbedder, SwapEmbedder, WrongDimsEmbedder, coordinator,
    dead_url, fake_vector, leaf, restore_legacy_format_row, store,
};
use crate::report::RebuildTargets;
use crate::worker::{DEFAULT_JOB_BATCH, DEFAULT_MAX_ATTEMPTS, IndexCoordinator, RETRY_BACKOFF_MS};
use clio_store::{EMBEDDING_DIMS, IndexStore, Store};
use clio_types::{AmError, SearchDomain};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Leaf write + worker → vector + lexical rows; leaf get works immediately.
#[test]
fn t01_leaf_write_then_worker_indexes_both_legs() {
    let store = store();
    let coordinator = coordinator(&store, Arc::new(FakeEmbedder::new()));
    let bank = "t01";
    let item = leaf("itm-1", bank, "deploy bot crashed with ECONNRESET");
    let t0 = Instant::now();
    store.create_memory_item(&item, "actor").unwrap();
    let create_ms = t0.elapsed().as_millis();
    // Leaf queryable immediately (enqueue is a bounded local INSERT).
    assert!(store.get_memory_item(bank, "itm-1").unwrap().is_some());
    assert!(
        create_ms < 1000,
        "write ACK must not wait on embedding: {create_ms}ms"
    );

    // FakeEmbedder health probe works.
    assert!(coordinator.health().is_ok());

    let report = coordinator.process_pending(10).unwrap();
    assert_eq!(report.applied, 1);
    assert_eq!(report.failed, 0);

    // Shapes through the primitives.
    let query = fake_vector("deploy bot crashed with ECONNRESET", EMBEDDING_DIMS);
    let dense = clio_store::IndexStore::dense_search(&*store, bank, &query, &[], 5).unwrap();
    assert_eq!(dense.first().expect("dense hit").item_id, "itm-1");
    let lexical =
        clio_store::IndexStore::lexical_search(&*store, bank, "ECONNRESET", &[], 5).unwrap();
    assert_eq!(lexical.first().expect("lexical hit").item_id, "itm-1");

    let status = coordinator.index_status(Some(bank)).unwrap();
    assert_eq!(status.coverage.dense_rows, 1);
    assert_eq!(status.coverage.lexical_rows, 1);
    assert_eq!(status.coverage.pending_jobs, 0);
    assert_eq!(status.embed.model_id, clio_store::DEFAULT_EMBEDDING_MODEL);
    assert!(status.embed.last_latency_ms.is_some());
}

/// Sidecar down → leaf write succeeds, pending flagged, backoff holds;
/// restore sidecar → rebuild clears the backlog.
#[test]
fn t02_sidecar_down_pending_remains_and_rebuild_recovers() {
    let store = store();
    let swap: Arc<SwapEmbedder> = Arc::new(SwapEmbedder::new(Arc::new(FailEmbedder)));
    let coordinator = coordinator(&store, Arc::clone(&swap) as Arc<dyn crate::embed::Embedder>);
    let bank = "t02";
    store
        .create_memory_item(&leaf("itm-2", bank, "sidecar outage test"), "actor")
        .unwrap();

    let report = coordinator.process_pending(10).unwrap();
    assert_eq!(report.failed, 1);
    // Leaf readable despite the outage.
    assert!(store.get_memory_item(bank, "itm-2").unwrap().is_some());
    // Job still pending with the error recorded.
    let status = coordinator.index_status(Some(bank)).unwrap();
    assert!(status.coverage.pending_jobs >= 1);
    assert!(status.coverage.last_error.is_some());
    // Health probe reflects the outage through the swap wrapper.
    assert!(coordinator.health().is_err());
    // Backoff: an immediate second sweep does not crash-loop.
    let report2 = coordinator.process_pending(10).unwrap();
    assert_eq!(report2.applied, 0);
    assert_eq!(report2.failed, 0, "failed row sits out the backoff window");

    // Restore the sidecar; rebuild (the documented recovery path) clears the
    // backlog, including the backoff-throttled row.
    swap.swap(Arc::new(FakeEmbedder::new()));
    let apply = coordinator
        .rebuild_indexes(Some(bank), RebuildTargets::All, false, true)
        .unwrap();
    assert_eq!(apply.dense_applied, 1);
    assert_eq!(apply.lexical_applied, 1);
    let status = coordinator.index_status(Some(bank)).unwrap();
    assert_eq!(status.coverage.pending_jobs, 0);
    assert_eq!(status.coverage.dense_rows, 1);
    assert_eq!(status.coverage.lexical_rows, 1);
    // Health probe reflects the restored sidecar.
    assert!(coordinator.health().is_ok());
}

/// Write ACK is never blocked by a slow embed sidecar.
#[test]
fn t11_write_ack_not_blocked_by_slow_embed() {
    let store = store();
    // 200ms delay > 50ms embedder timeout; Drop joins quickly.
    let server = FakeEmbedServer::spawn_slow(Duration::from_millis(200));
    let mut embedder = TeiEmbedder::new(server.url.clone(), "fake", EMBEDDING_DIMS, None).unwrap();
    embedder.set_timeout(Duration::from_millis(50));
    let coordinator = coordinator(&store, Arc::new(embedder));
    let bank = "t11";
    let t0 = Instant::now();
    store
        .create_memory_item(&leaf("itm-11", bank, "slow sidecar leaf"), "actor")
        .unwrap();
    let create_ms = t0.elapsed();
    assert!(
        create_ms < Duration::from_millis(500),
        "create must not wait on embed: {create_ms:?}"
    );
    assert!(store.get_memory_item(bank, "itm-11").unwrap().is_some());
    // Worker, not the write path, eats the timeout.
    let report = coordinator.process_pending(10).unwrap();
    assert_eq!(report.failed, 1);
}

/// Dimension mismatches fail closed with clear errors.
#[test]
fn t08_dim_mismatch_rejected() {
    // Config-side mismatch at construction (the store's active space is 384).
    let store = store();
    let tei = TeiEmbedder::new("http://127.0.0.1:34311", "wrong-model", 768, None).unwrap();
    let bad = IndexCoordinator::new(
        Arc::clone(&store) as Arc<dyn Store>,
        Arc::clone(&store) as Arc<dyn IndexStore>,
        Arc::new(tei),
        10,
    )
    .unwrap_err();
    assert!(bad.to_string().contains("active embedding space"), "{bad}");
    assert!(bad.to_string().contains("384 dims"), "{bad}");
    // Response-side mismatch fails the job without mutating anything.
    let coordinator = coordinator(&store, Arc::new(WrongDimsEmbedder));
    assert!(coordinator.health().is_ok());
    let bank = "t08";
    store
        .create_memory_item(&leaf("itm-8", bank, "dim mismatch"), "actor")
        .unwrap();
    let report = coordinator.process_pending(10).unwrap();
    assert_eq!(report.failed, 1);
    let jobs = store.pending_index_jobs(10, None, u32::MAX).unwrap();
    assert_eq!(jobs.len(), 1);
    assert!(
        jobs[0]
            .last_error
            .as_deref()
            .expect("clear error")
            .contains("dim mismatch fails closed")
    );
    assert!(store.get_memory_item(bank, "itm-8").unwrap().is_some());
    // Vector rows were never written.
    let status = coordinator.index_status(Some(bank)).unwrap();
    assert_eq!(status.coverage.dense_rows, 0);
}

/// Exhausted attempts stay pending (no silent drop) and legacy rows skip.
#[test]
fn exhausted_and_legacy_jobs_are_handled() {
    let store = store();
    let failing = IndexCoordinator::new(
        Arc::clone(&store) as Arc<dyn Store>,
        Arc::clone(&store) as Arc<dyn IndexStore>,
        Arc::new(FailEmbedder),
        1,
    )
    .unwrap();
    assert!(format!("{failing:?}").contains("max_attempts: 1"));
    let bank = "exh";
    store
        .create_memory_item(&leaf("itm-ex", bank, "exhausted"), "actor")
        .unwrap();
    restore_legacy_format_row(&store, bank, "env-1");
    let report = failing.process_pending(10).unwrap();
    assert_eq!(report.failed, 1);
    // Forced sweep ignores the backoff window, but the row already sits at the
    // retry cap, so it is not due: the sweep leaves it pending and untouched
    // instead of handing it a batch slot.
    let forced = failing.process_pending_forced(10).unwrap();
    assert_eq!(forced.skipped, 0, "a capped row is not due: {forced:?}");
    assert_eq!(forced.applied, 0, "{forced:?}");
    let pending = store.pending_index_jobs(10, None, u32::MAX).unwrap();
    assert!(
        pending
            .iter()
            .any(|j| j.bank_id == bank && j.item_id == "itm-ex"),
        "a capped row stays pending for status and rebuild"
    );

    // A restored legacy-format row never enqueues on write, but a manual one skips.
    assert!(failing.enqueue_leaf(bank, "env-1").unwrap());
    let report = failing.process_pending_forced(10).unwrap();
    assert_eq!(report.skipped, 1, "the legacy row skips and drains");
    let pending2 = store.pending_index_jobs(10, None, u32::MAX).unwrap();
    assert!(
        pending2.iter().all(|j| j.item_id != "env-1"),
        "legacy rows are skipped and drained, not errored"
    );
    let _ = RETRY_BACKOFF_MS;
}

/// Search primitives validate banks/limits/domains (fail closed).
#[test]
fn search_primitives_validate_args() {
    let store = store();
    let err = crate::search::dense_search(
        &*store,
        &crate::search::DenseSearchArgs {
            bank: "",
            query: &fake_vector("q", EMBEDDING_DIMS),
            domains: vec![],
            limit: 5,
        },
    )
    .unwrap_err();
    assert_eq!(err.code, clio_types::ErrorCode::MissingBank);
    let err = crate::search::lexical_search(
        &*store,
        &crate::search::LexicalSearchArgs {
            bank: "b",
            query: "x",
            domains: vec!["bad-domain".into()],
            limit: 5,
        },
    )
    .unwrap_err();
    assert_eq!(err.code, clio_types::ErrorCode::InvalidArgument);
    let err = crate::search::dense_search(
        &*store,
        &crate::search::DenseSearchArgs {
            bank: "b",
            query: &fake_vector("q", EMBEDDING_DIMS),
            domains: vec![],
            limit: 0,
        },
    )
    .unwrap_err();
    assert!(err.to_string().contains("limit"));
    let _ = AmError::new(clio_types::ErrorCode::Internal, "unused");
}

/// Dead-port embed client keeps failing clearly through the coordinator.
#[test]
fn dead_sidecar_reports_sidecar_down() {
    let store = store();
    let embedder = TeiEmbedder::new(dead_url(), "fake", EMBEDDING_DIMS, None).unwrap();
    let coordinator = coordinator(&store, Arc::new(embedder));
    let bank = "dead";
    store
        .create_memory_item(&leaf("itm-dead", bank, "dead sidecar"), "actor")
        .unwrap();
    let report = coordinator.process_pending(10).unwrap();
    assert_eq!(report.failed, 1);
    let pending = store.pending_index_jobs(10, None, u32::MAX).unwrap();
    assert!(pending.iter().any(|j| j.item_id == "itm-dead"));
    assert!(coordinator.health().is_err());
}

/// Rebuild sweep embeds feed the rolling worker observability state: a failing
/// sweep embed populates `last_error` (surfaced by `index_status`).
#[test]
fn rebuild_embed_failure_feeds_worker_state() {
    let store = store();
    let coordinator = coordinator(&store, Arc::new(FailEmbedder));
    let bank = "obs";
    store
        .create_memory_item(&leaf("itm-obs", bank, "observe rebuild"), "actor")
        .unwrap();
    assert!(
        coordinator
            .rebuild_indexes(Some(bank), RebuildTargets::Dense, false, true)
            .is_err()
    );
    let status = coordinator.index_status(Some(bank)).unwrap();
    assert!(
        status.embed.last_error.is_some(),
        "rebuild embed failure must populate worker last_error"
    );
}

/// Bank isolation through the coordinator-driven pipeline.
#[test]
fn t10_bank_isolation_end_to_end() {
    let store = store();
    let coordinator = coordinator(&store, Arc::new(FakeEmbedder::new()));
    store
        .create_memory_item(
            &leaf("itm-iso", "bank-iso", "isolated gist ECONNRESET"),
            "actor",
        )
        .unwrap();
    coordinator.process_pending(10).unwrap();
    let query = fake_vector("isolated gist ECONNRESET", EMBEDDING_DIMS);
    let dense =
        clio_store::IndexStore::dense_search(&*store, "other-bank", &query, &[], 5).unwrap();
    assert!(dense.is_empty(), "{dense:?}");
    let lexical =
        clio_store::IndexStore::lexical_search(&*store, "other", "ECONNRESET", &[], 5).unwrap();
    assert!(lexical.is_empty(), "{lexical:?}");
    let _ = SearchDomain::parse("task").unwrap();
}

/// No embedder configured → lexical doc is written, the job
/// finishes, and coverage reports dense unavailable (not silently empty).
#[test]
fn t35_lexical_only_drain_writes_lexical_and_reports_dense_unavailable() {
    let store = store();
    let coordinator = IndexCoordinator::new_lexical_only(
        Arc::clone(&store) as Arc<dyn Store>,
        Arc::clone(&store) as Arc<dyn IndexStore>,
        10,
    )
    .unwrap();
    assert!(!coordinator.dense_available());
    let bank = "lexonly";
    store
        .create_memory_item(&leaf("itm-lx", bank, "lexical only ECONNRESET"), "actor")
        .unwrap();

    let report = coordinator.process_pending(10).unwrap();
    assert_eq!(report.lexical_only, 1, "{report:?}");
    assert_eq!(report.applied, 0);
    assert_eq!(report.failed, 0);

    let lexical =
        clio_store::IndexStore::lexical_search(&*store, bank, "ECONNRESET", &[], 5).unwrap();
    assert_eq!(lexical.first().expect("lexical hit").item_id, "itm-lx");
    let status = coordinator.index_status(Some(bank)).unwrap();
    assert_eq!(status.coverage.lexical_rows, 1);
    assert_eq!(status.coverage.dense_rows, 0);
    assert_eq!(status.coverage.pending_jobs, 0);
    assert!(!status.embed.dense_available);
    assert_eq!(status.embed.dims, 0);
    // Health probe reports the missing provider rather than pretending healthy.
    assert!(coordinator.health().is_err());
}

/// A dense failure still leaves the lexical doc written, keeps the job
/// pending for retry with the error recorded, and never marks it fully indexed.
#[test]
fn t35_dense_failure_keeps_lexical_doc_and_leaves_job_pending() {
    let store = store();
    let coordinator = coordinator(&store, Arc::new(FailEmbedder));
    let bank = "densefail";
    store
        .create_memory_item(&leaf("itm-df", bank, "dense fails ECONNRESET"), "actor")
        .unwrap();

    let report = coordinator.process_pending(10).unwrap();
    assert_eq!(report.failed, 1, "{report:?}");
    assert_eq!(report.applied, 0);

    // Lexical doc survived the dense failure.
    let lexical =
        clio_store::IndexStore::lexical_search(&*store, bank, "ECONNRESET", &[], 5).unwrap();
    assert_eq!(lexical.first().expect("lexical hit").item_id, "itm-df");
    let status = coordinator.index_status(Some(bank)).unwrap();
    assert_eq!(status.coverage.lexical_rows, 1);
    assert_eq!(status.coverage.dense_rows, 0);

    // Job is still queued with attempts/last_error recorded (not finished).
    let pending = store.pending_index_jobs(10, None, u32::MAX).unwrap();
    let job = pending
        .iter()
        .find(|j| j.item_id == "itm-df")
        .expect("job stays pending");
    assert_eq!(job.attempts, 1);
    assert!(job.last_error.is_some());
}

/// Regression for #14: rows at the retry cap used to be handed to every sweep and
/// counted as skipped, so a full batch of them starved newer work forever. They
/// now leave the due set, stay visible in status, and a rebuild re-derives them.
#[test]
fn exhausted_jobs_do_not_starve_newer_work() {
    let store = store();
    let coordinator = coordinator(&store, Arc::new(FakeEmbedder::new()));
    let bank = "exhausted";

    // A full batch of capped rows, created before the new item.
    for i in 0..DEFAULT_JOB_BATCH {
        let id = format!("itm-exhausted-{i}");
        store
            .create_memory_item(&leaf(&id, bank, "sidecar outage"), "actor")
            .unwrap();
        for _ in 0..DEFAULT_MAX_ATTEMPTS {
            store
                .fail_index_job(bank, &id, "embed sidecar down")
                .unwrap();
        }
    }
    store
        .create_memory_item(&leaf("itm-fresh", bank, "fresh work"), "actor")
        .unwrap();

    // A forced sweep ignores the retry window, which is the state the backlog
    // reaches once its last attempt is older than the window. The capped rows must
    // not consume the batch ahead of the newer item.
    let sweep = coordinator
        .process_pending_forced(DEFAULT_JOB_BATCH)
        .unwrap();
    assert_eq!(
        sweep.applied, 1,
        "a newer job must not be starved: {sweep:?}"
    );
    assert_eq!(sweep.skipped, 0, "{sweep:?}");

    // The capped rows stay pending and visible until a rebuild re-derives them.
    let status = coordinator.index_status(Some(bank)).unwrap();
    assert_eq!(status.coverage.pending_jobs, DEFAULT_JOB_BATCH as u64);
    assert_eq!(status.coverage.retry_jobs, DEFAULT_JOB_BATCH as u64);
    assert!(status.coverage.last_error.is_some());
}
