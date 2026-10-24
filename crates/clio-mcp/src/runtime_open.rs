// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for constructing [`McpState`] over a concrete
//! store backend: backend selection, provider wiring, and engine assembly.
//!
//! ## Owns
//! - Backend dispatch (sqlite / postgres) and embedding-space reconciliation
//! - One embedder resolution shared by retrieval and the index drain
//! - The retriever, its read-only diagnostic twin, and the hub/association/
//!   maintenance engine wiring
//! - Extraction-readiness derivation, the caller-supplied erasure-journal
//!   path, and the retention-store loader
//!
//! ## Does not own
//! - The `McpState` type and its public `open` API (see `runtime`)
//! - Tool dispatch or handler semantics (see `write_tools` and friends)
//! - Admission or retrieval math (domain crates own those)
//!
//! ## Boundary
//!
//! Changes to which engines a store is wired with should normally require
//! changes here. Changes to the `McpState` field set should keep both modules
//! in sync.
//!
//! Keep this module focused on runtime state construction.

use crate::index_drain::{INDEX_MAX_BATCH, IndexDrain};
use crate::ops_embedder::{OpsEmbedConfig, build_ops_embedder_from};
use crate::runtime::{McpOpenOptions, McpState, PipelineStatus};
use crate::runtime_repos::Repos;
use clio_admission::RankingEnv;
use clio_compliance::{InventoryBackend, InventoryFacts};
use clio_config::retention_store::RetentionStore;
use clio_retrieve::{AssociationEngine, HybridRetriever};
use clio_store::AssocPolicy;
use clio_store::{
    BankScopeStore, BatchStore, BeliefStore, ComplianceStore, ContinuousStore, CopyInventoryStore,
    DekProvider, DestinationCustodyStore, DurableRequestStore, HistoryStore, HygieneStore,
    IndexStore, LocalDevKms, LocalEventStore, OpsStore, PeerActionStore, PersonaStore,
    PostgresStore, SqliteFileState, SqliteStore, Store, SyncStore,
};
use clio_types::{AmError, ErrorCode};
use clio_write::{HubDistillEngine, LeafIndex, MemForest, MemTreeMaint};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[cfg(test)]
#[path = "runtime_open_tests.rs"]
mod runtime_open_tests;

/// The caller's retriever and its read-only diagnostic twin: both from the
/// same env, store, index, embedder, and reranker, with the twin's
/// co-activation reinforcement disabled, so a read-only report ranks the
/// caller's page without durably reinforcing it. Reinforcement never changes
/// the current page (only future ones), so the twin returns the identical page.
fn retriever_pair(
    store: &Arc<dyn Store>,
    index: &Arc<dyn IndexStore>,
    env: &RankingEnv,
    embedder: Option<&Arc<dyn clio_index::Embedder>>,
    reranker: Option<&Arc<dyn clio_retrieve::Reranker>>,
) -> (Arc<HybridRetriever>, Arc<HybridRetriever>) {
    let mut retriever =
        HybridRetriever::with_ranking_env(Arc::clone(store), Arc::clone(index), env);
    let mut diagnostic =
        HybridRetriever::with_ranking_env(Arc::clone(store), Arc::clone(index), env);
    if let Some(embedder) = embedder {
        retriever.set_embedder(Arc::clone(embedder));
        diagnostic.set_embedder(Arc::clone(embedder));
    }
    if let Some(reranker) = reranker {
        retriever.set_reranker(Arc::clone(reranker));
        diagnostic.set_reranker(Arc::clone(reranker));
    }
    diagnostic.set_reinforcement(false);
    (Arc::new(retriever), Arc::new(diagnostic))
}

impl McpState {
    /// Open runtime state over a concrete store backend, keeping `runtime` as
    /// the effective-config `Runtime` served by the config/ranking tools.
    ///
    /// `journal_path` is already resolved by the constructor: the deployment
    /// constructors pass `None`, and only a host-supplied document names one.
    pub(crate) fn open_with_runtime_and_effective(
        options: &McpOpenOptions,
        runtime: clio_config::Runtime,
        effective: &Value,
        journal_path: Option<PathBuf>,
    ) -> Result<Arc<Self>, AmError> {
        Self::open_with_runtime_effective_and_wave(options, runtime, effective, false, journal_path)
    }

    pub(crate) fn open_with_runtime_effective_and_wave(
        options: &McpOpenOptions,
        runtime: clio_config::Runtime,
        effective: &Value,
        join_consolidate_wave: bool,
        journal_path: Option<PathBuf>,
    ) -> Result<Arc<Self>, AmError> {
        let kms: Arc<dyn DekProvider> = match &options.kms_path {
            Some(path) => Arc::new(LocalDevKms::open_persistent(path)?),
            None => Arc::new(LocalDevKms::new()),
        };
        match options.backend.as_str() {
            "sqlite" => {
                let mut store = if options.database_url == "sqlite::memory:" {
                    SqliteStore::open_in_memory(Arc::clone(&kms))?
                } else {
                    SqliteStore::open(Path::new(&options.database_url), Arc::clone(&kms))?
                };
                reconcile_space(&mut store, effective)?;
                let store = Arc::new(store);
                let belief_corrector: Arc<dyn clio_compliance::BeliefContextCorrector> =
                    store.clone();
                let store: Arc<dyn Repos> = store;
                Self::build(
                    &store,
                    belief_corrector,
                    options,
                    effective,
                    kms,
                    runtime,
                    join_consolidate_wave,
                    journal_path,
                )
            }
            "postgres" => {
                let mut store = PostgresStore::open(&options.database_url, kms.clone())?;
                reconcile_space(&mut store, effective)?;
                let store = Arc::new(store);
                let belief_corrector: Arc<dyn clio_compliance::BeliefContextCorrector> =
                    store.clone();
                let store: Arc<dyn Repos> = store;
                Self::build(
                    &store,
                    belief_corrector,
                    options,
                    effective,
                    kms,
                    runtime,
                    join_consolidate_wave,
                    journal_path,
                )
            }
            other => Err(AmError::new(
                ErrorCode::OutOfRange,
                format!("backend must be sqlite|postgres, got `{other}`"),
            )),
        }
    }

    #[allow(clippy::too_many_lines)]
    #[expect(clippy::too_many_arguments, reason = "field set and seam together")]
    fn build(
        store: &Arc<dyn Repos>,
        belief_corrector: Arc<dyn clio_compliance::BeliefContextCorrector>,
        options: &McpOpenOptions,
        effective: &Value,
        kms: Arc<dyn DekProvider>,
        config: clio_config::Runtime,
        join_consolidate_wave: bool,
        journal_path: Option<PathBuf>,
    ) -> Result<Arc<Self>, AmError> {
        let env = RankingEnv::system_defaults();
        let store_any: Arc<dyn Store> = store.clone();
        let index: Arc<dyn IndexStore> = store.clone();
        let persona: Arc<dyn PersonaStore> = store.clone();
        let belief: Arc<dyn BeliefStore> = store.clone();
        let history: Arc<dyn HistoryStore> = store.clone();
        let continuous: Arc<dyn ContinuousStore> = store.clone();
        let compliance: Arc<dyn ComplianceStore> = store.clone();
        let hygiene: Arc<dyn HygieneStore> = store.clone();
        let events: Arc<dyn LocalEventStore> = store.clone();
        let durable: Arc<dyn DurableRequestStore> = store.clone();
        let peer_actions: Arc<dyn PeerActionStore> = store.clone();
        let copies: Arc<dyn CopyInventoryStore> = store.clone();
        let bank_scope: Arc<dyn BankScopeStore> = store.clone();
        let custody: Arc<dyn DestinationCustodyStore> = store.clone();
        let batch: Arc<dyn BatchStore> = store.clone();
        let ops_store: Arc<dyn OpsStore> = store.clone();
        let sync: Arc<dyn SyncStore> = store.clone();
        let (inventory_facts, inventory_backend, inventory_measured) = measure_inventory(options)?;
        // One embedder resolution, shared by retrieval and the index drain.
        let ops_embedder = build_ops_embedder_from(&OpsEmbedConfig::from_effective(
            effective,
            clio_config::config::bearer_for_effective(effective, "embed"),
        ))?;
        let reranker = clio_retrieve::build_reranker_from(
            effective,
            clio_config::config::bearer_for_effective(effective, "rerank"),
        )?;
        let rerank_active = reranker.is_some();
        let (retriever, diagnostic_retriever) = retriever_pair(
            &store_any,
            &index,
            &env,
            ops_embedder.as_ref(),
            reranker.as_ref(),
        );
        let index_drain = Arc::new(IndexDrain::new(
            Arc::clone(&store_any),
            Arc::clone(&index),
            ops_embedder.as_ref().map(Arc::clone),
            INDEX_MAX_BATCH,
        )?);
        let dense_available = index_drain.dense_available();
        let extract_available = extract_available_from(effective);
        // Each engine derives its co-activation policy from the same ranking
        // environment through the one conversion, instead of hardcoding
        // `AssocPolicy::default()`. At open the environment is the system
        // default, so these values are unchanged. A later `ranking_env_set`
        // governs the next hub wave and the next retrieval alike: every one of
        // these engines holds a replaceable policy cell, pushed by
        // `McpState::apply_ranking_policy`.
        let assoc = AssociationEngine::new(
            Arc::clone(&store_any),
            AssocPolicy::from_coactivation(&env.coactivation),
        );
        let leaves = Arc::new(LeafIndex::new());
        let forest: Arc<std::sync::Mutex<MemForest>> =
            Arc::new(std::sync::Mutex::new(MemForest::new()));
        let maint = MemTreeMaint::new(Arc::clone(&forest), Arc::clone(&leaves));
        let hub = Arc::new(HubDistillEngine::new(
            Arc::clone(&store_any),
            env.clone(),
            AssocPolicy::from_coactivation(&env.coactivation),
        ));
        let state = Arc::new(Self {
            persona,
            belief,
            belief_corrector,
            history,
            continuous,
            compliance,
            hygiene,
            events,
            durable,
            peer_actions,
            copies,
            bank_scope,
            custody,
            inventory_facts: Arc::new(inventory_facts),
            inventory_measured,
            inventory_backend,
            batch,
            ops_store,
            store: store_any,
            index,
            retriever,
            diagnostic_retriever,
            env,
            assoc,
            maint,
            hub: Some(hub),
            leaves,
            forest,
            default_bank: options.default_bank.clone(),
            default_actor: options.default_actor,
            budget_tokens: 2048,
            history_policy: clio_history::HistoryPolicy::default(),
            scratchpad: Arc::new(clio_compliance::Scratchpad::new()),
            shared_enabled: options.shared_bank_enabled,
            ops_plans: Arc::new(clio_ops::PlanRegistry::new()),
            ops_embedder,
            index_drain,
            pipeline: PipelineStatus {
                dense_available,
                rerank_active,
                extract_available,
            },
            index_wake: Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new())),
            maintenance_trigger: Arc::new(
                crate::maintenance_trigger::MaintenanceTriggerState::default(),
            ),
            join_consolidate_wave,
            sync,
            kms,
            device_id: format!("device-{}", std::process::id()),
            retention: Arc::new(std::sync::Mutex::new(load_retention_store())),
            retention_path: clio_config::retention_store::resolve_retention_path(None),
            journal_path,
            config: Arc::new(std::sync::Mutex::new(config)),
            id_counter: Arc::new(std::sync::atomic::AtomicU64::new(1)),
            // Unbound at construction: only a credential-bearing transport ever
            // attaches a principal, and it does so per request.
            principal: None,
            #[cfg(test)]
            test_now: None,
        });
        if let Err(err) = reconcile_after_open(&state) {
            eprintln!("clio-mcp: export destination reconcile failed: {err}");
        }
        Ok(state)
    }
}

/// Reconcile any open export destinations left by a restart.
///
/// Best-effort at open: a refusal of the copy controls or a read failure must
/// never crash the runtime, so the caller logs and continues.
fn reconcile_after_open(state: &McpState) -> Result<(), clio_types::AmError> {
    clio_compliance::reconcile_open_destinations(&*state.events, &*state.custody).map(|_| ())
}

/// Live-store facts for a subject-scoped export, measured once at open.
///
/// A measurement failure fails the open with the measurement's own error code,
/// so a caller never sees a silent empty inventory behind a successful open: an
/// inventory it cannot measure is a store it cannot export from, and an open
/// that reported success would let a subject export emit a wrong manifest or
/// claim there was nothing to export. Only a `sqlite::memory:` store opens
/// unmeasured — it has no file to measure — and that empty measurement is
/// marked unmeasured so a subject export refuses with `InvalidArgument`.
fn measure_inventory(
    options: &McpOpenOptions,
) -> Result<(InventoryFacts, InventoryBackend, bool), AmError> {
    let empty_sqlite = || {
        InventoryFacts::Sqlite(SqliteFileState {
            path: PathBuf::new(),
            exists: false,
            bytes: 0,
            table_counts: Vec::new(),
            banks: Vec::new(),
        })
    };
    if options.backend.as_str() == "postgres" {
        let state = clio_store::postgres_target_state(&options.database_url).map_err(|err| {
            AmError::new(
                err.code,
                format!("measure the postgres inventory at open: {}", err.message),
            )
        })?;
        return Ok((
            InventoryFacts::Postgres(state),
            InventoryBackend::Postgres,
            true,
        ));
    }
    if options.database_url == "sqlite::memory:" {
        return Ok((empty_sqlite(), InventoryBackend::Sqlite, false));
    }
    let state = clio_store::sqlite_file_state(Path::new(&options.database_url)).map_err(|err| {
        AmError::new(
            err.code,
            format!("measure the sqlite inventory at open: {}", err.message),
        )
    })?;
    Ok((
        InventoryFacts::Sqlite(state),
        InventoryBackend::Sqlite,
        true,
    ))
}

/// Whether the configured extraction provider would actually run, matching the
/// fail-closed rules of `clio_write::build_extractor_from` (readiness).
///
/// The `template` provider needs only a URL; hosted `openai` also needs a
/// non-empty model and a credential, because `ingest_raw` fails closed without
/// them. Any other provider value is rejected at ingest time, so it reports
/// not-available rather than a readiness false positive.
pub(crate) fn extract_available_from(effective: &Value) -> bool {
    let text = |pointer: &str| {
        effective
            .pointer(pointer)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
    };
    let url = text("/extract/url");
    if url.is_empty() {
        return false;
    }
    match text("/extract/provider") {
        "" | "template" => true,
        "openai" => {
            !text("/extract/model").is_empty()
                && clio_config::config::bearer_for_effective(effective, "extract").is_some()
        }
        _ => false,
    }
}

/// Reconcile a freshly opened store with the configured embedding space.
///
/// The space switch is destructive to derived vectors, so it only runs when the
/// effective config opts in (`embed.switch_space`); otherwise a mismatch fails
/// closed with operator guidance.
fn reconcile_space(
    store: &mut impl clio_store::SpaceReconcile,
    effective: &Value,
) -> Result<(), AmError> {
    let space = clio_store::configured_embedding_space(effective)?;
    store.reconcile_space(
        space.as_ref(),
        clio_store::space_switch_confirmed(effective),
    )?;
    Ok(())
}

/// Load the durable retention profile store from the process environment.
///
/// An unconfigured path, or an unreadable/corrupt file, yields the builtin-only
/// store: a bad stored profile must never block admission.
#[must_use]
pub fn load_retention_store() -> RetentionStore {
    load_retention_store_from(clio_config::retention_store::resolve_retention_path(None))
}

/// Injectable variant of [`load_retention_store`] (one codegen path for coverage).
#[must_use]
pub(crate) fn load_retention_store_from(path: Option<std::path::PathBuf>) -> RetentionStore {
    match path {
        Some(p) => RetentionStore::load_lossy(&p),
        None => RetentionStore::default(),
    }
}
