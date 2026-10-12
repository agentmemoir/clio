// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the index maintenance coordinator: outbox
//! worker, rebuild hooks, and PII-safe pipeline status.
//!
//! ## Owns
//! - `IndexCoordinator` (drains `index_pending`; applies dense + lexical)
//! - `rebuild_indexes` (dry-run + confirmed apply; sweep + orphan cleanup)
//! - `index_status` coverage/status report for later ops surfaces
//!
//! ## Does not own
//! - Backend index SQL (`clio_store::IndexStore`)
//! - Embedding transport (`embed`/`http`)
//! - Hybrid retrieval or compose (owned by the retrieval layer)
//!
//! ## Boundary
//!
//! Changes to worker/rebuild policy should normally require changes here.
//! Changes to backend SQL or search primitives should not require changes here.
//!
//! # Failure isolation
//!
//! The worker never rolls back an admitted leaf: embed outages mark the job
//! pending with a bounded backoff and a structured `last_error`; leaves stay
//! readable by id the whole time. Rebuild restores coverage
//! after an outage or intentional wipe: derived rows are safe to re-derive
//! (exact-KNN scale ceiling is accepted for this store generation).

use crate::embed::Embedder;
use crate::report::{EmbedStatus, IndexStatus, ProcessReport, RebuildReport, RebuildTargets};
use clio_store::{IndexJob, IndexStore, Store, VectorSlot};
use clio_types::{AmError, ErrorCode, MemoryItem};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// Retry backoff window (epoch-millis) before a failed job is retried.
pub const RETRY_BACKOFF_MS: u64 = 30_000;
/// Default jobs drained per `process_pending` call.
pub const DEFAULT_JOB_BATCH: usize = 32;
/// Failed attempts after which a job leaves the due set: it stays pending with
/// its error visible to `index_status` until a rebuild re-derives it.
pub const DEFAULT_MAX_ATTEMPTS: u32 = 10;

/// Rolling process-local observability (best-effort; never persisted).
#[derive(Debug, Default)]
struct WorkerState {
    last_latency_ms: Option<u128>,
    last_error: Option<String>,
}

/// How one drained job resolved.
///
/// Dense-unavailable and dense-failed are deliberately distinct: a lexical-only
/// job is complete (no embedder configured), while a dense failure leaves the
/// job pending for retry with its lexical doc already written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JobOutcome {
    /// Dense vector + lexical doc written.
    Applied,
    /// Lexical doc written; no embedder configured for the dense leg.
    LexicalOnly,
    /// Nothing to index (item gone or a legacy opaque envelope).
    Skipped,
}

/// Drains the index outbox and rebuilds derived indexes for one backend.
pub struct IndexCoordinator {
    store: Arc<dyn Store>,
    index: Arc<dyn IndexStore>,
    embedder: Option<Arc<dyn Embedder>>,
    max_attempts: u32,
    state: Mutex<WorkerState>,
}

impl std::fmt::Debug for IndexCoordinator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IndexCoordinator")
            .field("model_id", &self.embedder.as_ref().map(|e| e.model_id()))
            .field("dims", &self.embedder.as_ref().map(|e| e.dims()))
            .field("max_attempts", &self.max_attempts)
            .finish_non_exhaustive()
    }
}

impl IndexCoordinator {
    /// Build a coordinator; fails closed on a dims mismatch with the active space.
    ///
    /// The store resolves the active embedding space at bootstrap; a configured
    /// embedder with any other dimension is rejected here, not at upsert time.
    ///
    /// # Errors
    /// Embedder dims != the store's active embedding width.
    pub fn new(
        store: Arc<dyn Store>,
        index: Arc<dyn IndexStore>,
        embedder: Arc<dyn Embedder>,
        max_attempts: u32,
    ) -> Result<Self, AmError> {
        Self::new_optional(store, index, Some(embedder), max_attempts)
    }

    /// Build a lexical-only coordinator: no embedder is configured, so the
    /// dense leg is skipped and reported as unavailable (never silently empty).
    ///
    /// # Errors
    /// Storage failures resolving the active embedding space.
    pub fn new_lexical_only(
        store: Arc<dyn Store>,
        index: Arc<dyn IndexStore>,
        max_attempts: u32,
    ) -> Result<Self, AmError> {
        Self::new_optional(store, index, None, max_attempts)
    }

    fn new_optional(
        store: Arc<dyn Store>,
        index: Arc<dyn IndexStore>,
        embedder: Option<Arc<dyn Embedder>>,
        max_attempts: u32,
    ) -> Result<Self, AmError> {
        if let Some(embedder) = &embedder {
            let dims = embedder.dims();
            let space = store.embedding_space()?;
            if dims != space.dims() {
                return Err(AmError::new(
                    ErrorCode::InvalidArgument,
                    format!(
                        "embed model {} produces {dims} dims; the active embedding space is {} @ {} \
                         dims (re-configure embed.dims and run a dense rebuild to switch models)",
                        embedder.model_id(),
                        space.model_id(),
                        space.dims()
                    ),
                ));
            }
        }
        Ok(Self {
            store,
            index,
            embedder,
            max_attempts,
            state: Mutex::new(WorkerState::default()),
        })
    }

    /// `true` when a dense embedder is configured for this coordinator.
    #[must_use]
    pub fn dense_available(&self) -> bool {
        self.embedder.is_some()
    }

    /// Sidecar health probe.
    ///
    /// # Errors
    /// Sidecar unreachable or unhealthy, or no embedder configured.
    pub fn health(&self) -> Result<(), AmError> {
        match &self.embedder {
            Some(embedder) => embedder.health(),
            None => Err(AmError::new(
                ErrorCode::NotImplemented,
                "no embedding provider configured (lexical-only index mode)",
            )),
        }
    }

    /// Manually enqueue one item (write paths enqueue transactionally already).
    ///
    /// # Errors
    /// Storage failures.
    pub fn enqueue_leaf(&self, bank_id: &str, item_id: &str) -> Result<bool, AmError> {
        self.index.enqueue_index_job(bank_id, item_id)
    }

    /// Drain up to `max_jobs` due outbox rows: decrypt → scrub → embed →
    /// dense upsert + lexical doc. Never touches a leaf's readability.
    ///
    /// Rows that failed recently sit out the retry backoff window; pass
    /// [`Self::process_pending_forced`] to ignore the window (ops retry-now).
    ///
    /// # Errors
    /// Queue read/write failures (per-job failures land on the job row).
    pub fn process_pending(&self, max_jobs: usize) -> Result<ProcessReport, AmError> {
        let cutoff = epoch_millis().saturating_sub(RETRY_BACKOFF_MS).to_string();
        self.drain(max_jobs, Some(&cutoff))
    }

    /// Like [`Self::process_pending`] but ignoring the retry backoff window.
    ///
    /// # Errors
    /// Queue read/write failures (per-job failures land on the job row).
    pub fn process_pending_forced(&self, max_jobs: usize) -> Result<ProcessReport, AmError> {
        self.drain(max_jobs, None)
    }

    /// Drain the one pending job for `(bank_id, item_id)`, if it is still queued.
    ///
    /// A one-shot write calls this so the item it just stored is searchable even
    /// when an older backlog fills the bounded [`Self::process_pending`] batch.
    /// The same due rules apply: a row retried inside the backoff window and one
    /// at `max_attempts` are skipped. An item with no queued job is a no-op.
    ///
    /// # Errors
    /// Queue read/write failures (per-job failures land on the job row).
    pub fn process_item(&self, bank_id: &str, item_id: &str) -> Result<ProcessReport, AmError> {
        let cutoff = epoch_millis().saturating_sub(RETRY_BACKOFF_MS).to_string();
        let Some(job) =
            self.index
                .pending_index_job(bank_id, item_id, Some(&cutoff), self.max_attempts)?
        else {
            return Ok(ProcessReport::default());
        };
        self.process_jobs(vec![job])
    }

    fn drain(&self, max_jobs: usize, cutoff: Option<&str>) -> Result<ProcessReport, AmError> {
        // The due query never returns rows at `max_attempts`: the worker does not
        // retry them, so they must not hold batch slots ahead of newer jobs (the
        // failure this replaced let a full batch of exhausted rows starve new
        // work forever). They stay pending and visible in coverage/status until a
        // rebuild re-derives them or `cleanup_orphans` drops them with their item.
        let jobs = self
            .index
            .pending_index_jobs(max_jobs, cutoff, self.max_attempts)?;
        self.process_jobs(jobs)
    }

    /// Process one already-selected set of due jobs.
    fn process_jobs(&self, jobs: Vec<IndexJob>) -> Result<ProcessReport, AmError> {
        let mut report = ProcessReport::default();
        for job in jobs {
            match self.process_one(&job) {
                Ok(JobOutcome::Applied) => {
                    self.index.finish_index_job(&job.bank_id, &job.item_id)?;
                    report.applied += 1;
                }
                Ok(JobOutcome::LexicalOnly) => {
                    self.index.finish_index_job(&job.bank_id, &job.item_id)?;
                    report.lexical_only += 1;
                }
                Ok(JobOutcome::Skipped) => {
                    self.index.finish_index_job(&job.bank_id, &job.item_id)?;
                    report.skipped += 1;
                }
                Err(err) => {
                    self.index
                        .fail_index_job(&job.bank_id, &job.item_id, &err.to_string())?;
                    self.record_error(&err);
                    report.failed += 1;
                }
            }
        }
        Ok(report)
    }

    /// Internal rebuild API. `dry_run=true` reports counts without writing;
    /// a mutating run requires `confirm=true` (the ops layer wires the UX around it).
    ///
    /// Apply sweeps every visible indexable item in scope: re-embeds (dense)
    /// and re-derives the lexical doc (idempotent upserts), then removes
    /// derived rows whose item vanished. After an intentional wipe everything
    /// is missing, so the sweep restores full coverage.
    ///
    /// # Errors
    /// Missing bank, refused confirmation, embed failures, or storage failures.
    pub fn rebuild_indexes(
        &self,
        bank: Option<&str>,
        targets: RebuildTargets,
        dry_run: bool,
        confirm: bool,
    ) -> Result<RebuildReport, AmError> {
        // Route sweep embeds through the observing wrapper so rebuild activity
        // keeps feeding the rolling latency/error state surfaced by `status`.
        let observed = self.embedder.as_ref().map(|embedder| ObservingEmbedder {
            inner: &**embedder,
            state: &self.state,
        });
        crate::rebuild::rebuild(
            &*self.store,
            &*self.index,
            observed.as_ref().map(|o| o as &dyn Embedder),
            bank,
            targets,
            dry_run,
            confirm,
        )
    }

    /// PII-safe pipeline status for the requested scope.
    ///
    /// # Errors
    /// Coverage query failures.
    pub fn index_status(&self, bank: Option<&str>) -> Result<IndexStatus, AmError> {
        let coverage = self.index.index_coverage(bank)?;
        let state = self.state_snapshot();
        let space = self.store.embedding_space()?;
        let (model_id, dims, dense_available) = match &self.embedder {
            Some(embedder) => (embedder.model_id(), embedder.dims(), true),
            None => (String::new(), 0, false),
        };
        Ok(IndexStatus {
            embed: EmbedStatus {
                model_id,
                dims,
                dense_available,
                index_mode: space.index_mode().as_str().to_owned(),
                last_latency_ms: state.last_latency_ms,
                last_error: state.last_error,
            },
            coverage,
        })
    }

    /// Apply dense + lexical updates for one job.
    ///
    /// The lexical doc is written first and unconditionally, so a dense failure
    /// never discards it and never leaves the job marked fully indexed: the job
    /// is failed for retry with its lexical row already durable.
    ///
    /// `JobOutcome::Skipped` = nothing to index: item deleted while queued, or
    /// the row is a legacy-format row restored through the opaque
    /// ciphertext_backup route with no dual content.
    fn process_one(&self, job: &IndexJob) -> Result<JobOutcome, AmError> {
        let Some(item) = self.fetch_indexable_item(&job.bank_id, &job.item_id)? else {
            return Ok(JobOutcome::Skipped);
        };
        let Some(text) = crate::embed::embed_text(&item) else {
            return Ok(JobOutcome::Skipped);
        };
        let lexical = crate::embed::lexical_text(&item).unwrap_or_else(|| text.clone());
        // The store keys the assembled text into per-term digests; no readable
        // word reaches the stored column.
        self.index
            .put_lexical_doc(&job.bank_id, &job.item_id, &lexical)?;
        let Some(embedder) = &self.embedder else {
            return Ok(JobOutcome::LexicalOnly);
        };
        let vectors = self.embed_and_observe(embedder.as_ref(), &[text])?;
        let vector = vectors
            .first()
            .expect("one input yields one vector")
            .clone();
        self.store.upsert_vector(&VectorSlot {
            item_id: job.item_id.clone(),
            bank_id: job.bank_id.clone(),
            model_id: embedder.model_id(),
            embedding: vector,
        })?;
        Ok(JobOutcome::Applied)
    }

    /// Fetch one item for indexing: `None` when it is gone, invalidated to a
    /// deleted state, or unreadable (nothing to derive from).
    ///
    /// Same skip rule as `rebuild::fetch_indexable_item`: an unreadable row is
    /// dropped, a broken backend errors.
    fn fetch_indexable_item(&self, bank: &str, id: &str) -> Result<Option<MemoryItem>, AmError> {
        match self.store.get_memory_item(bank, id) {
            Ok(item) => Ok(item),
            Err(e) if clio_store::is_unreadable_content(&e) => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn embed_and_observe(
        &self,
        embedder: &dyn Embedder,
        texts: &[String],
    ) -> Result<Vec<Vec<f32>>, AmError> {
        let t0 = Instant::now();
        let vectors = embedder.embed_batch(texts);
        match &vectors {
            Ok(vectors) => {
                // Fail closed on any dimension drift before storage sees it.
                for (i, vector) in vectors.iter().enumerate() {
                    if vector.len() != embedder.dims() {
                        return Err(AmError::new(
                            ErrorCode::OutOfRange,
                            format!(
                                "embed model {} returned {} dims for input {i}; expected {} \
                                 (dim mismatch fails closed; re-configure embed.dims and rebuild)",
                                embedder.model_id(),
                                vector.len(),
                                embedder.dims()
                            ),
                        ));
                    }
                }
                self.record_latency(t0.elapsed().as_millis());
            }
            Err(e) => self.record_error(e),
        }
        vectors
    }

    fn record_error(&self, err: &AmError) {
        let mut state = self.state.lock().expect("index coordinator state poisoned");
        state.last_error = Some(err.to_string());
    }

    fn record_latency(&self, ms: u128) {
        let mut state = self.state.lock().expect("index coordinator state poisoned");
        state.last_latency_ms = Some(ms);
    }

    fn state_snapshot(&self) -> WorkerState {
        let guard = self.state.lock().expect("index coordinator state poisoned");
        WorkerState {
            last_latency_ms: guard.last_latency_ms,
            last_error: guard.last_error.clone(),
        }
    }
}

fn epoch_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// [`Embedder`] wrapper that records sweep embed latency/error into the
/// coordinator's rolling worker state, preserving the observability
/// contract (`status().embed.last_latency_ms` / `last_error`) for rebuilds.
struct ObservingEmbedder<'a> {
    inner: &'a dyn Embedder,
    state: &'a Mutex<WorkerState>,
}

impl Embedder for ObservingEmbedder<'_> {
    fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, AmError> {
        let t0 = Instant::now();
        let vectors = self.inner.embed_batch(texts);
        match &vectors {
            Ok(_) => {
                let mut state = self.state.lock().expect("index coordinator state poisoned");
                state.last_latency_ms = Some(t0.elapsed().as_millis());
            }
            Err(e) => {
                let mut state = self.state.lock().expect("index coordinator state poisoned");
                state.last_error = Some(e.to_string());
            }
        }
        vectors
    }

    fn health(&self) -> Result<(), AmError> {
        self.inner.health()
    }

    fn model_id(&self) -> String {
        self.inner.model_id()
    }

    fn dims(&self) -> usize {
        self.inner.dims()
    }
}

#[cfg(test)]
#[path = "worker_tests.rs"]
mod worker_tests;
