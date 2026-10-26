// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for MemTree dirty-path coalesce and level-parallel refresh.
//!
//! ## Owns
//! - Coalesced dirty set keyed by `node_id` (drained per wave; re-marks stay queued)
//! - Bottom-up same-depth parallel summary refresh with bounded workers
//! - Generation-guarded refresh so mid-wave re-dirtied nodes are never cleared
//! - Per-bank timing for completed structural refresh waves
//! - The forest, leaf index and optional store handle the engine carries
//! - A wave gauge claimed by the spawner and released once the structural
//!   refresh completes, so a background wave counts from the moment it is
//!   scheduled rather than from when it runs
//! - A cumulative waves-started counter, so a wave that started and finished
//!   between two samples is still counted
//!
//! ## Does not own
//! - Tree topology / fan-out split algorithms
//! - Near-duplicate candidate consolidation (`consolidate` module)
//! - Agent-facing tool handlers (`memtree_tools`)
//! - The non-blocking counters and the contended dirty-ancestor read
//!   (`memtree_maint_reads`)
//! - Durable row persistence of attached and refreshed nodes (`memtree_hook`),
//!   which owns the upsert; this engine only carries the store handle
//!
//! ## Boundary
//!
//! Changes to dirty refresh / maintenance scheduling should normally require changes here.
//! Changes to tool shapes, leaf admit gates or row persistence should not require changes here.
//!
//! Keep this module focused on MemTree maintenance internals.

use crate::maintenance::LeafIndex;
use crate::memtree::{MemForest, MemTreeNode};
use clio_store::Store;
use memtree_refresh_one::refresh_one;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
#[cfg(test)]
use std::time::Duration;
use std::time::Instant;

/// Shared MemTree forest handle.
pub type SharedMemForest = Arc<Mutex<MemForest>>;

/// Coalesce + refresh instrumentation.
#[derive(Debug, Default)]
pub struct MaintStats {
    /// Times a dirty mark hit an already-queued node_id.
    pub coalesce_hits: AtomicU64,
    /// Distinct nodes refreshed successfully.
    pub refreshed: AtomicU64,
    /// Nodes left dirty after refresh failure.
    pub refresh_failures: AtomicU64,
}

/// Maintenance engine over a shared forest.
pub struct MemTreeMaint {
    pub(crate) forest: SharedMemForest,
    pub(crate) leaves: Arc<LeafIndex>,
    /// Durable sink for attached and refreshed rows (`None` stays in-process).
    pub(crate) store: Option<Arc<dyn Store>>,
    /// Coalesced dirty set: bank → node_ids
    dirty_set: Mutex<HashMap<String, HashSet<String>>>,
    /// Optional node ids that fail refresh (tests).
    fail_nodes: Mutex<HashSet<String>>,
    stats: MaintStats,
    /// Most recently completed structural refresh duration, keyed by bank.
    last_structural_maintenance_ms: Mutex<HashMap<String, u128>>,
    /// Controlled delay used only to make refresh timing tests deterministic.
    #[cfg(test)]
    test_refresh_delay_ms: AtomicU64,
    /// Wave slots claimed by a spawner, so a test can wait for the claimed
    /// waves to finish.
    ///
    /// Always compiled, not test-only: the spawner has to claim its slot before
    /// it spawns, and the spawner is production code. The
    /// [`MemTreeMaint::waves_in_flight`] accessor samples it, and
    /// [`MemTreeMaint::wait_for_quiescence`] waits on it.
    waves_in_flight: AtomicUsize,
    /// Cumulative count of waves begun, including waves that already finished.
    waves_started: AtomicU64,
    /// Job counter for consolidate accept status.
    pub(crate) jobs: AtomicU64,
}

/// Outcome of one node refresh attempt.
pub(crate) enum RefreshOutcome {
    /// Summary recomputed and node cleared.
    Refreshed,
    /// Node was re-dirtied during the wave; stays queued for the next wave.
    Superseded,
    /// Refresh failed; node stays dirty with the error recorded.
    Failed(String),
}

impl MemTreeMaint {
    /// Build a maintainer bound to a forest and leaf index.
    #[must_use]
    pub fn new(forest: SharedMemForest, leaves: Arc<LeafIndex>) -> Arc<Self> {
        Self::with_store_handle(forest, leaves, None)
    }

    /// The one constructor body; [`MemTreeMaint::with_store`] adds the sink.
    pub(crate) fn with_store_handle(
        forest: SharedMemForest,
        leaves: Arc<LeafIndex>,
        store: Option<Arc<dyn Store>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            forest,
            leaves,
            store,
            dirty_set: Mutex::new(HashMap::new()),
            fail_nodes: Mutex::new(HashSet::new()),
            stats: MaintStats::default(),
            last_structural_maintenance_ms: Mutex::new(HashMap::new()),
            #[cfg(test)]
            test_refresh_delay_ms: AtomicU64::new(0),
            waves_in_flight: AtomicUsize::new(0),
            waves_started: AtomicU64::new(0),
            jobs: AtomicU64::new(1),
        })
    }

    /// Test aid: force refresh failure for a node.
    pub fn fail_refresh_node(&self, node_id: impl Into<String>) {
        self.fail_nodes
            .lock()
            .expect("fail set")
            .insert(node_id.into());
    }

    /// Delay subsequent refresh waves for deterministic metric tests.
    #[cfg(test)]
    pub fn set_test_refresh_delay(&self, delay: Duration) {
        let delay_ms = u64::try_from(delay.as_millis()).unwrap_or(u64::MAX);
        self.test_refresh_delay_ms
            .store(delay_ms, Ordering::Relaxed);
    }

    /// Enqueue dirty ancestors into the coalesce set.
    pub fn enqueue_dirty(&self, bank_id: &str, node_ids: &[String]) {
        let mut guard = self.dirty_set.lock().expect("dirty set");
        let set = guard.entry(bank_id.to_owned()).or_default();
        for id in node_ids {
            if !set.insert(id.clone()) {
                self.stats.coalesce_hits.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// Detach erased leaves and dirty their ancestor chains (compliance erasure).
    ///
    /// For every erased item id, the leaf node is removed from the tree and the
    /// process-local leaf cache, and each surviving ancestor is marked dirty with
    /// its aggregate summary cleared. No stale ancestor summary survives until
    /// the next [`MemTreeMaint::refresh_bank`] wave; callers that need immediate
    /// recomputation should call `refresh_bank` right after.
    pub fn detach_erased(&self, bank_id: &str, item_ids: &[String]) -> Vec<String> {
        let mut dirty = Vec::new();
        {
            let mut forest = self.forest.lock().expect("forest");
            if forest.tree(bank_id).is_none() {
                return dirty;
            }
            let tree = forest.tree_mut(bank_id);
            for item_id in item_ids {
                let _ = self.leaves.remove(bank_id, item_id);
                let Some(node_id) = tree.placement.remove(item_id) else {
                    continue;
                };
                let ancestors = tree.ancestors_of(&node_id);
                tree.nodes.remove(&node_id);
                for aid in ancestors {
                    if let Some(n) = tree.nodes.get_mut(&aid) {
                        n.dirty = true;
                        n.generation = n.generation.saturating_add(1);
                        n.summary_gist = None;
                        dirty.push(aid);
                    }
                }
            }
        }
        self.enqueue_dirty(bank_id, &dirty);
        dirty
    }

    /// Deterministic aggregate gist from children (CI fixture mode).
    #[must_use]
    pub fn summarize_children(children: &[&MemTreeNode]) -> String {
        let parts: Vec<&str> = children
            .iter()
            .map(|c| {
                c.summary_gist
                    .as_deref()
                    .or(c.item_id.as_deref())
                    .unwrap_or("?")
            })
            .take(8)
            .collect();
        format!("aggregate[{}]: {}", children.len(), parts.join(" | "))
    }

    /// Drain the queued dirty ids for a bank. Marks enqueued during the wave
    /// therefore survive for the next wave instead of being dropped.
    fn drain_dirty(&self, bank_id: &str) -> HashSet<String> {
        let mut guard = self.dirty_set.lock().expect("dirty set");
        guard.remove(bank_id).unwrap_or_default()
    }

    /// Claim an in-flight wave slot before scheduling the work.
    ///
    /// [`MemTreeMaint::wait_for_quiescence`] is only correct if a scheduled
    /// wave is already visible in the gauge. A spawner that increments the
    /// gauge *inside* the new thread leaves a gap: the spawn call returns, the
    /// gauge still reads zero, and a waiter concludes the queue is idle while
    /// the wave is still pending. It then races a wave it believes has
    /// finished, and the two waves drain the same coalesced set against each
    /// other. Claim first, run second.
    pub fn begin_wave(&self) {
        // The cumulative counter is stored before the gauge's release, so a
        // quiescence wait that acquires the gauge also observes the started
        // count for that wave.
        self.waves_started.fetch_add(1, Ordering::Relaxed);
        self.waves_in_flight.fetch_add(1, Ordering::AcqRel);
    }

    /// Release a slot claimed by [`MemTreeMaint::begin_wave`].
    pub fn end_wave(&self) {
        self.waves_in_flight.fetch_sub(1, Ordering::AcqRel);
    }

    /// How many waves have ever begun, including waves that already finished.
    ///
    /// Cumulative rather than an in-flight gauge, so a wave that started and
    /// finished between two samples still counts: a gauge sampled after the
    /// call would read zero for it. Reading it performs no work and starts no
    /// wave.
    #[must_use]
    pub fn waves_started(&self) -> u64 {
        self.waves_started.load(Ordering::Relaxed)
    }

    /// Block until no refresh wave is running or pending.
    ///
    /// A wave requeues whatever it could not finish, so a caller that polls the
    /// dirty set while one is still in flight competes with that wave for the
    /// forest lock and for the machine's cores. The wave can then outlast a
    /// wall-clock budget that assumed it was the only work in flight. A test
    /// that asserts convergence waits here first, which turns the convergence
    /// check into a state check instead of a race against a thread it cannot
    /// join.
    #[cfg(test)]
    pub fn wait_for_quiescence(&self) {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let in_flight = self.waves_in_flight.load(Ordering::Acquire);
            if in_flight == 0 {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "refresh wave did not finish within 60s; waves_in_flight={in_flight}"
            );
            thread::sleep(Duration::from_millis(1));
        }
    }

    /// How many wave slots are claimed right now.
    ///
    /// Non-blocking: a relaxed load of the gauge, so any thread can read it
    /// without waiting for a wave to finish, which is what
    /// [`MemTreeMaint::wait_for_quiescence`] is for. Reading it starts no wave.
    ///
    /// The slot covers scheduling and the structural refresh only. A spawner
    /// claims it with [`MemTreeMaint::begin_wave`] before it spawns, and
    /// releases it with [`MemTreeMaint::end_wave`] once the wave's structural
    /// refresh has returned. Hub distillation runs after that release, so a
    /// wave distilling hubs reads zero here: this gauge does not report hub
    /// work.
    #[must_use]
    pub fn waves_in_flight(&self) -> usize {
        self.waves_in_flight.load(Ordering::Relaxed)
    }

    /// Level-parallel bottom-up refresh for one bank. Returns cleared node ids.
    pub fn refresh_bank(&self, bank_id: &str) -> Vec<String> {
        self.begin_wave();
        let cleared = self.refresh_bank_inner(bank_id);
        self.end_wave();
        cleared
    }

    /// The wave body, without the in-flight gauge.
    ///
    /// Public for cross-crate spawners that claim the slot themselves with
    /// [`MemTreeMaint::begin_wave`] and release it with
    /// [`MemTreeMaint::end_wave`], in the background-wave execution pattern.
    pub fn refresh_bank_inner(&self, bank_id: &str) -> Vec<String> {
        let pending = self.drain_dirty(bank_id);
        if pending.is_empty() {
            return vec![];
        }

        // (id, generation at wave start) grouped by depth; refresh ascending
        // (leaves depth 0 first — usually clean).
        let by_depth: BTreeMap<u32, Vec<(String, u64)>> = {
            let forest = self.forest.lock().expect("forest");
            let Some(tree) = forest.tree(bank_id) else {
                return vec![];
            };
            let mut map: BTreeMap<u32, Vec<(String, u64)>> = BTreeMap::new();
            for id in &pending {
                if let Some(n) = tree.get(id)
                    && n.dirty
                {
                    map.entry(n.depth)
                        .or_default()
                        .push((id.clone(), n.generation));
                }
            }
            map
        };

        let started = Instant::now();
        #[cfg(test)]
        {
            let delay_ms = self.test_refresh_delay_ms.load(Ordering::Relaxed);
            if delay_ms > 0 {
                thread::sleep(Duration::from_millis(delay_ms));
            }
        }
        let cleared = self.refresh_dirty_depths(bank_id, by_depth);
        let elapsed_ms = started.elapsed().as_millis();
        self.last_structural_maintenance_ms
            .lock()
            .expect("maintenance timing")
            .insert(bank_id.to_owned(), elapsed_ms);
        cleared
    }

    /// Execute one pre-snapshotted refresh wave and requeue unfinished nodes.
    fn refresh_dirty_depths(
        &self,
        bank_id: &str,
        by_depth: BTreeMap<u32, Vec<(String, u64)>>,
    ) -> Vec<String> {
        let mut cleared = Vec::new();
        let mut keep_queued = Vec::new();
        let mut persist = Vec::new();
        let fail = self.fail_nodes.lock().expect("fail set").clone();

        for (_depth, ids) in by_depth {
            // Same-depth parallel refresh with a bounded worker pool: at most
            // min(CPU budget P, dirty count at this level) threads are live.
            let workers = thread::available_parallelism()
                .map_or(1, NonZeroUsize::get)
                .min(ids.len())
                .max(1);
            let results: Mutex<Vec<(String, RefreshOutcome)>> = Mutex::new(Vec::new());
            let next = AtomicUsize::new(0);
            thread::scope(|scope| {
                let ids = &ids;
                for _ in 0..workers {
                    let forest = Arc::clone(&self.forest);
                    let fail = fail.clone();
                    let results = &results;
                    let next = &next;
                    scope.spawn(move || {
                        loop {
                            let i = next.fetch_add(1, Ordering::Relaxed);
                            let Some((id, wave_gen)) = ids.get(i) else {
                                break;
                            };
                            let outcome = refresh_one(&forest, bank_id, id, *wave_gen, &fail);
                            results.lock().expect("results").push((id.clone(), outcome));
                        }
                    });
                }
            });

            for (id, outcome) in results.into_inner().expect("results") {
                match outcome {
                    RefreshOutcome::Refreshed => {
                        persist.push(id.clone());
                        cleared.push(id);
                        self.stats.refreshed.fetch_add(1, Ordering::Relaxed);
                    }
                    // Superseded and failed nodes stay dirty and queued so the
                    // next wave retries them instead of dropping the mark.
                    other => {
                        if let RefreshOutcome::Failed(msg) = other {
                            persist.push(id.clone());
                            self.stats.refresh_failures.fetch_add(1, Ordering::Relaxed);
                            let mut forest = self.forest.lock().expect("forest");
                            if let Some(n) = forest.tree_mut(bank_id).node_mut(&id) {
                                n.dirty = true;
                                n.refreshing = false;
                                n.last_error = Some(msg);
                            }
                        }
                        keep_queued.push(id);
                    }
                }
            }
        }

        self.enqueue_dirty(bank_id, &keep_queued);
        // A refreshed node now carries the new gist with `dirty` cleared, and a
        // failed one carries its error; both are what a reader must see.
        self.persist_nodes(bank_id, &persist);
        cleared
    }
}

#[path = "memtree_hook.rs"]
mod memtree_hook;

#[path = "memtree_maint_reads.rs"]
mod memtree_maint_reads;

#[path = "memtree_refresh_one.rs"]
mod memtree_refresh_one;

#[cfg(test)]
#[path = "memtree_maint_erase_tests.rs"]
mod memtree_maint_erase_tests;

#[cfg(test)]
#[path = "memtree_maint_wave_tests.rs"]
mod memtree_maint_wave_tests;

#[cfg(test)]
#[path = "memtree_persist_tests.rs"]
mod memtree_persist_tests;
