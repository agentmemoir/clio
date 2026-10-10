// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `hub_distill` maintenance job: flagging
//! over-degree association hubs and distilling each hub cluster into one gated
//! semantic item (category + five-factor admission), with cross-references and
//! optional cluster-edge prune.
//!
//! ## Owns
//! - `HubDistillEngine`, hub eligibility, and the distill pipeline
//! - `distill_hub_under`: one hub cluster distilled into one gated semantic
//!   item, and the per-hub outcome statuses it reports
//! - The per-bank in-flight counter map a wave claims its slot in
//!   (`hub_distill_wave`)
//!
//! ## Does not own
//! - The wave envelope and its per-wave hub budget (`hub_distill_wave`)
//! - Distill report/item shapes and pure helpers (`hub_distill_model`)
//! - Association storage (`clio-store`) or admission scoring (`clio-admission`)
//! - MemTree dirty-path refresh (`memtree_maint`)
//!
//! ## Boundary
//!
//! Changes to hub detection/distillation should normally require changes here.
//! Changes to admission math or assoc storage should not require changes here.

use self::hub_distill_freshness::RetainedWaveTallies;
use self::hub_distill_reconcile::erasure_dead;
use crate::hub_distill_claim::{annotate_resumed, claim_before_work, complete_claim_with_revision};
use crate::hub_distill_model::{
    HubSignalSource, build_distill_item, build_distill_record, modal_category, neighbor_text,
};
use clio_admission::decision::Candidate;
use clio_admission::gated::{GateRequest, gated_create};
use clio_admission::policy::AdmissionPolicy;
use clio_admission::{EventSink, VecSink};
use clio_config::ranking::RankingEnv;
use clio_config::retention::ResolvedRetention;
use clio_store::{AssocPolicy, Store};
use clio_types::{AmError, ErrorCode, MemoryItem, OpContext};
use std::collections::HashMap;
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex, RwLock, RwLockReadGuard};
use std::time::{SystemTime, UNIX_EPOCH};

pub use self::hub_distill_wave::BudgetedWave;
pub use crate::hub_distill_model::{
    DEFAULT_HUB_NEIGHBORS, DistillOutcome, HUB_DISTILL_JOB, HUB_DISTILL_MARKER, HubDistillReport,
    StaleTally,
};

/// Hub distillation engine over one store.
///
/// The co-activation policy sits behind a lock rather than in a plain field
/// because the ranking tools tune it for the process's own config session, and
/// a wave started after that tuning must see the new value. The engine stays
/// policy-driven: it never reads configuration itself, and a caller supplies
/// the policy it wants the next wave to use through [`Self::set_policy`].
pub struct HubDistillEngine {
    store: Arc<dyn Store>,
    env: RankingEnv,
    policy: RwLock<AssocPolicy>,
    now_millis: Option<i64>,
    prune_after_distill: bool,
    neighbor_limit: usize,
    sink: Arc<dyn EventSink + Send + Sync>,
    /// The last productive wave's stale tally per bank, retained between
    /// waves and bounded at 64 banks (see `hub_distill_freshness`).
    last_wave_stale: Mutex<RetainedWaveTallies>,
    /// How many hub-distillation waves are running per bank right now. Keyed
    /// by bank, so one bank's wave is never counted as another bank's work
    /// (see `hub_distill_wave`).
    hub_waves_in_flight: Mutex<HashMap<String, Arc<AtomicUsize>>>,
}

impl std::fmt::Debug for HubDistillEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HubDistillEngine")
            .field("policy", &*self.policy())
            .field("prune_after_distill", &self.prune_after_distill)
            .finish_non_exhaustive()
    }
}

impl HubDistillEngine {
    /// Build an engine using the system clock and a fresh in-memory sink.
    #[must_use]
    pub fn new(store: Arc<dyn Store>, env: RankingEnv, policy: AssocPolicy) -> Self {
        Self {
            store,
            env,
            policy: RwLock::new(policy),
            now_millis: None,
            prune_after_distill: true,
            neighbor_limit: DEFAULT_HUB_NEIGHBORS,
            sink: Arc::new(VecSink::default()),
            last_wave_stale: Mutex::new(RetainedWaveTallies::default()),
            hub_waves_in_flight: Mutex::new(HashMap::new()),
        }
    }

    /// Build an engine with a fixed clock (deterministic tests).
    #[must_use]
    pub fn with_clock(
        store: Arc<dyn Store>,
        env: RankingEnv,
        policy: AssocPolicy,
        now_millis: i64,
    ) -> Self {
        let mut engine = Self::new(store, env, policy);
        engine.now_millis = Some(now_millis);
        engine
    }

    /// Enable/disable pruning of cluster edges after a successful distill.
    pub fn set_prune_after_distill(&mut self, enabled: bool) {
        self.prune_after_distill = enabled;
    }

    /// Replace the admission accept/reject sink.
    pub fn set_sink(&mut self, sink: Arc<dyn EventSink + Send + Sync>) {
        self.sink = sink;
    }

    /// The policy the next wave will use.
    fn policy(&self) -> RwLockReadGuard<'_, AssocPolicy> {
        self.policy.read().expect("hub policy lock poisoned")
    }

    /// Replace the co-activation policy used by waves started after this call.
    ///
    /// A wave takes one snapshot when it starts and reads every scan from that
    /// one value, so a change made while a wave is in flight lands on the next
    /// wave and one wave never mixes two knob sets.
    pub fn set_policy(&self, policy: AssocPolicy) {
        *self.policy.write().expect("hub policy lock poisoned") = policy;
    }

    /// The policy value a wave starting now will use.
    ///
    /// Cloned rather than held: a read guard kept across a wave would block
    /// `set_policy` for the whole wave, and `RwLock::read` documents that it
    /// may panic if the same thread already holds the lock.
    fn policy_snapshot(&self) -> AssocPolicy {
        self.policy().clone()
    }

    fn now_millis(&self) -> i64 {
        self.now_millis.unwrap_or_else(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        })
    }

    /// Node ids whose above-threshold effective coactivation degree exceeds the
    /// configured `hub_degree_limit` (read-only; no decay prune here).
    ///
    /// # Errors
    /// Empty bank or storage failures.
    pub fn eligible_hubs(&self, bank: &str) -> Result<Vec<String>, AmError> {
        let policy = self.policy_snapshot();
        self.eligible_hubs_under(bank, &policy)
    }

    /// Distill one hub cluster into a gated semantic item under `retention`, the
    /// bank's resolved retention profile.
    ///
    /// # Errors
    /// Empty ids, missing hub, or storage failures. Admission reject is `Ok`.
    pub fn distill_hub(
        &self,
        bank: &str,
        hub_id: &str,
        ctx: &OpContext,
        retention: ResolvedRetention,
    ) -> Result<DistillOutcome, AmError> {
        let policy = self.policy_snapshot();
        self.distill_hub_under(bank, hub_id, ctx, retention, &policy)
    }

    /// [`Self::distill_hub`] under the policy its wave already snapshotted, so
    /// every hub in one wave is scanned with the same value.
    ///
    /// # Errors
    /// Empty ids, missing hub, or storage failures. Admission reject is `Ok`.
    #[allow(clippy::too_many_lines)]
    fn distill_hub_under(
        &self,
        bank: &str,
        hub_id: &str,
        ctx: &OpContext,
        retention: ResolvedRetention,
        policy: &AssocPolicy,
    ) -> Result<DistillOutcome, AmError> {
        if bank.is_empty() {
            return Err(AmError::new(
                ErrorCode::MissingBank,
                "hub distill requires a bank",
            ));
        }
        let item_id = format!("hub-distill:{bank}:{hub_id}");
        // The claim is an evidence trail only: it never routes. Whether this
        // wave resumed a claim an earlier wave left incomplete only annotates
        // the outcome below.
        let resumed = claim_before_work(
            self.store.as_ref(),
            bank,
            hub_id,
            &item_id,
            ctx.actor.as_str(),
            &self.now_millis().to_string(),
        )?;
        let hub = match self.store.get_memory_item(bank, hub_id) {
            Ok(Some(hub)) => hub,
            Ok(None) => {
                return Err(AmError::new(
                    ErrorCode::ItemNotFound,
                    format!("hub `{hub_id}` not found in bank"),
                ));
            }
            // A hub whose content erasure made unreadable carries a purged
            // unit: report it instead of failing the wave.
            Err(err) if erasure_dead(&err) => {
                return Ok(DistillOutcome {
                    hub_id: hub_id.to_owned(),
                    item_id,
                    status: "purged".into(),
                    admission_score: None,
                    reason: Some("hub content was purged by erasure".into()),
                });
            }
            Err(err) => return Err(err),
        };
        match self.store.get_memory_item(bank, &item_id) {
            // The unit exists. With a derivation record the lifecycle knows it:
            // this call reconciles it against the current cluster (keep a
            // distilled unit current instead of leaving it frozen). A pre-record
            // row stays `already_distilled`. A complete claim changes nothing
            // here: an unchanged cluster reconciles with no writes, which is
            // the no-op.
            Ok(Some(_)) => {
                let mut outcome = match self.store.get_distilled_unit(bank, &item_id)? {
                    Some(record) => {
                        // The unit is already here, so an earlier wave's work
                        // landed. Finish its claim with the stored record's
                        // revision: the completion write only lands while the
                        // claim is still incomplete, so a complete claim's
                        // no-write no-op here is untouched. It runs after the
                        // reconcile, so a failed reconcile still leaves the
                        // claim incomplete for the next wave.
                        let revision = record.current_revision_item_id();
                        let outcome = self.reconcile_unit(bank, record, ctx, retention, policy)?;
                        complete_claim_with_revision(self.store.as_ref(), bank, hub_id, &revision)?;
                        outcome
                    }
                    None => DistillOutcome {
                        hub_id: hub_id.to_owned(),
                        item_id,
                        status: "already_distilled".into(),
                        admission_score: None,
                        reason: None,
                    },
                };
                annotate_resumed(&mut outcome, resumed);
                return Ok(outcome);
            }
            Ok(None) => {}
            // The unit's content erasure purged: report it, never fail the
            // wave and never resurrect it.
            Err(err) if erasure_dead(&err) => {
                return Ok(DistillOutcome {
                    hub_id: hub_id.to_owned(),
                    item_id,
                    status: "purged".into(),
                    admission_score: None,
                    reason: Some("unit content was purged by erasure".into()),
                });
            }
            Err(err) => return Err(err),
        }
        let neighbors = self.top_neighbors(bank, hub_id, policy)?;
        if neighbors.is_empty() {
            return Ok(DistillOutcome {
                hub_id: hub_id.to_owned(),
                item_id,
                status: "no_neighbors".into(),
                admission_score: None,
                reason: Some("hub has no above-threshold neighbors".into()),
            });
        }
        let category = modal_category(&hub, &neighbors);
        let ts = self.now_millis().to_string();
        let record = build_distill_record(bank, &hub, &item_id, &neighbors, &ts);
        let item = build_distill_item(bank, &hub, category, &record, &neighbors, &ts);
        let source = HubSignalSource {
            texts: neighbors.iter().map(neighbor_text).collect(),
        };
        let decision =
            self.gate_distill(&item, category, &record, &source, ctx, &retention, &ts)?;
        self.audit(bank, hub_id, &item_id, &neighbors, decision.pass, ctx)?;
        if !decision.pass {
            return Ok(DistillOutcome {
                hub_id: hub_id.to_owned(),
                item_id,
                status: "rejected".into(),
                admission_score: Some(decision.admission_score),
                reason: decision.rejection_reason,
            });
        }
        self.cross_reference(bank, &item_id, hub_id, &neighbors, ctx)?;
        if self.prune_after_distill {
            self.prune_cluster(bank, hub_id, ctx)?;
        }
        // The claim completes last, so a completed claim always means the whole
        // unit including its edges landed. Refusals and errors return above and
        // leave the claim incomplete for the next wave to re-evaluate.
        complete_claim_with_revision(
            self.store.as_ref(),
            bank,
            hub_id,
            &record.current_revision_item_id(),
        )?;
        let mut outcome = DistillOutcome {
            hub_id: hub_id.to_owned(),
            item_id,
            status: "distilled".into(),
            admission_score: Some(decision.admission_score),
            reason: None,
        };
        annotate_resumed(&mut outcome, resumed);
        Ok(outcome)
    }

    /// Run one first distillation through the admission gate. The item and its
    /// derivation record land in one store transaction, so an admitted distill is
    /// never stored as an untracked unit.
    ///
    /// # Errors
    /// Storage failures and gate failures; an admission reject is `Ok`.
    #[allow(clippy::too_many_arguments)]
    fn gate_distill(
        &self,
        item: &MemoryItem,
        category: clio_types::SemanticCategory,
        record: &clio_store::DistilledUnitRecord,
        source: &HubSignalSource,
        ctx: &OpContext,
        retention: &ResolvedRetention,
        ts: &str,
    ) -> Result<clio_admission::decision::Decision, AmError> {
        let admission = AdmissionPolicy::with_retention(&self.env, retention.clone());
        let mut write = |admitted: &MemoryItem| {
            self.store
                .commit_distilled_unit(admitted, record, ctx.actor.as_str())
        };
        gated_create(
            &GateRequest {
                item,
                ctx,
                policy: admission,
                sink: self.sink.as_ref(),
                timestamp: ts,
            },
            Candidate::Semantic(category),
            source,
            &mut write,
        )
    }
}

#[path = "hub_distill_wave.rs"]
mod hub_distill_wave;

#[path = "hub_distill_freshness.rs"]
mod hub_distill_freshness;

#[path = "hub_distill_scan.rs"]
mod hub_distill_scan;

#[path = "hub_distill_edges.rs"]
mod hub_distill_edges;

#[path = "hub_distill_reconcile.rs"]
mod hub_distill_reconcile;

#[cfg(test)]
#[path = "hub_distill_tests.rs"]
pub(crate) mod hub_distill_tests;

#[cfg(test)]
#[path = "hub_distill_consolidate_force_tests.rs"]
mod hub_distill_consolidate_force_tests;

#[cfg(test)]
#[path = "hub_distill_reconcile_edge_tests.rs"]
mod hub_distill_reconcile_edge_tests;
#[cfg(test)]
#[path = "hub_distill_reconcile_lifecycle_tests.rs"]
mod hub_distill_reconcile_lifecycle_tests;

#[cfg(test)]
#[path = "hub_distill_reconcile_tests.rs"]
mod hub_distill_reconcile_tests;

#[cfg(test)]
#[path = "hub_distill_policy_tests.rs"]
mod hub_distill_policy_tests;

#[cfg(test)]
#[path = "hub_distill_record_tests.rs"]
mod hub_distill_record_tests;

#[cfg(test)]
#[path = "hub_distill_evidence_tests.rs"]
mod hub_distill_evidence_tests;

#[cfg(test)]
#[path = "hub_distill_crash_tests.rs"]
mod hub_distill_crash_tests;
