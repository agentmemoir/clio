// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the MCP runtime state: the shared `McpState`
//! type and its public open API that the transports hand to the tool
//! dispatchers.
//!
//! ## Owns
//! - The `McpState` type, its field set, and the public `open` entry points
//! - Effective-config resolution policy (`open` vs `open_with_effective`),
//!   including the document-named erasure-journal seam
//! - Bank/actor defaults, the shared-bank permission gate, and the
//!   administrative-mutation gate
//! - stderr telemetry sink adapters (events never touch stdout)
//!
//! ## Does not own
//! - Store-backend construction and engine wiring (see `runtime_open`)
//! - Tool dispatch logic (see `write_tools`)
//! - Admission, storage, or write-path semantics
//! - Per-call context/id/timestamp helpers (see `runtime_context`)
//! - Index-drain scheduling and status (see `runtime_index` / `index_drain`)
//!
//! ## Boundary
//!
//! Changes to the `McpState` field set or its open API should normally require
//! changes here. Changes to which engines a store is wired with should not
//! require changes here.
//!
//! Keep this module focused on the MCP runtime state and its open API.

use clio_admission::RankingEnv;
use clio_config::retention_store::RetentionStore;
use clio_history::HistoryPolicy;
use clio_index::Embedder;
use clio_retrieve::{AssociationEngine, HybridRetriever};
use clio_store::{
    BatchStore, BeliefStore, ComplianceStore, ContinuousStore, CopyInventoryStore, DekProvider,
    DestinationCustodyStore, DurableRequestStore, HistoryStore, HygieneStore, IndexStore,
    LocalEventStore, OpsStore, PeerActionStore, PersonaStore, Store, SyncStore,
};
use clio_types::{Actor, AmError, ErrorCode};
use clio_write::{HubDistillEngine, LeafIndex, MemForest, MemTreeMaint};

use crate::index_drain::IndexDrain;
use crate::maintenance_trigger::MaintenanceTriggerState;
pub use crate::ops_embedder::{build_ops_embedder, build_ops_embedder_with};
use crate::principal::Principal;
use serde_json::Value;
use std::sync::{Arc, Condvar};

pub use crate::runtime_sinks::{BatchSignals, NoSignals, StderrSinks, StoreSignals};

/// Pinned protocol revision carried for diagnostics assertions.
pub const PROTOCOL_REVISION: &str = crate::MCP_PROTOCOL_REVISION;

/// Dedicated cross-agent shared bank namespace. Access is gated at
/// bank resolution so no tool can route around the deployment permission.
pub const SHARED_BANK: &str = "shared";

/// Shared state behind every MCP transport: one store, one engine set.
///
/// `Clone` is a shallow copy, and every field that must stay single and
/// unforked sits behind an `Arc`: the store engines, the index drain, the
/// retention store, and the config runtime. A clone therefore shares one
/// instance of each and can differ only in [`Self::principal`], which is what
/// makes a per-request principal cheap and safe to attach.
#[derive(Clone)]
pub struct McpState {
    /// Discrete/episodic repository.
    pub store: Arc<dyn Store>,
    /// Operator health view of the same backend (ops surfaces).
    pub ops_store: Arc<dyn OpsStore>,
    /// Index backend view (dense + lexical search) for read/retrieve tools.
    pub index: Arc<dyn IndexStore>,
    /// Hybrid retriever shared by `retrieve` and `compose_context`.
    pub retriever: Arc<HybridRetriever>,
    /// Read-only diagnostic twin of `retriever`: same env, store, index,
    /// embedder, and reranker, with co-activation reinforcement disabled.
    pub diagnostic_retriever: Arc<HybridRetriever>,
    /// Persona repository view of the same backend.
    pub persona: Arc<dyn PersonaStore>,
    /// Belief repository view.
    pub belief: Arc<dyn BeliefStore>,
    /// Authorized belief-context correction view (concrete-backend
    /// capability kept out of the shared read trait).
    pub belief_corrector: Arc<dyn clio_compliance::BeliefContextCorrector>,
    /// History repository view.
    pub history: Arc<dyn HistoryStore>,
    /// Continuous (EMA) repository view.
    pub continuous: Arc<dyn ContinuousStore>,
    /// Compliance-erasure persistence view.
    pub compliance: Arc<dyn ComplianceStore>,
    /// Hygiene persistence view (flags, archive markers, log).
    pub hygiene: Arc<dyn HygieneStore>,
    /// Durable local event writer for protected-action intents and outcomes.
    pub events: Arc<dyn LocalEventStore>,
    /// Durable subject-request records for the request service.
    pub durable: Arc<dyn DurableRequestStore>,
    /// Peer-action rows for subject requests.
    pub peer_actions: Arc<dyn PeerActionStore>,
    /// Copy-inventory rows for subject requests.
    pub copies: Arc<dyn CopyInventoryStore>,
    /// Live bank set an erasure fans out to for one data subject.
    pub bank_scope: Arc<dyn clio_store::BankScopeStore>,
    /// Export destination-custody persistence.
    pub custody: Arc<dyn DestinationCustodyStore>,
    /// Best-effort measured live-store facts for a subject-scoped export.
    pub inventory_facts: Arc<clio_compliance::InventoryFacts>,
    /// Whether [`Self::inventory_facts`] is a real measurement. A subject
    /// export refuses (`InvalidArgument`) rather than emit a wrong manifest.
    pub inventory_measured: bool,
    /// The store backend [`Self::inventory_facts`] describes.
    pub inventory_backend: clio_compliance::InventoryBackend,
    /// Admission/ranking environment snapshot.
    pub env: RankingEnv,
    /// Association engine (co-activation + explicit edges).
    pub assoc: AssociationEngine,
    /// `MemTree` maintenance engine ([`clio_write::MemTreeMaint`]).
    pub maint: Arc<MemTreeMaint>,
    /// Hub distillation engine ([`HubDistillEngine`]; `None` disables hub
    /// consolidation).
    pub hub: Option<Arc<HubDistillEngine>>,
    /// Published leaves for immediate queryability.
    pub leaves: Arc<LeafIndex>,
    /// Shared `MemTree` forest (bank-keyed [`clio_write::MemForest`]).
    pub forest: Arc<std::sync::Mutex<MemForest>>,
    /// Default bank when args omit one (args win).
    pub default_bank: String,
    /// Default actor when args omit one (args win).
    pub default_actor: Actor,
    /// Persona token budget for post-mutation truncation.
    pub budget_tokens: usize,
    /// Lesson cap / read-bound policy for history tools.
    pub history_policy: HistoryPolicy,
    /// Ephemeral scratchpad workspace (never durable, never indexed).
    ///
    /// Behind an `Arc` so a per-request state clone shares one scratchpad rather
    /// than handing each request an empty workspace of its own.
    pub scratchpad: Arc<clio_compliance::Scratchpad>,
    /// Atomic batch commit handle for the same backend.
    pub batch: Arc<dyn BatchStore>,
    /// Explicit deployment permission for the cross-agent shared bank.
    pub shared_enabled: bool,
    /// Process-local repair-plan registry for the ops `doctor`/`repair` surfaces.
    pub ops_plans: Arc<clio_ops::PlanRegistry>,
    /// Optional embedding sidecar for the ops `reindex`/`repair` surfaces.
    ///
    /// Also the one embedder shared by the index drain and the retrieval dense
    /// leg: no second embedder and no duplicate config reads.
    pub ops_embedder: Option<Arc<dyn Embedder>>,
    /// Background index drain over the durable outbox (always present).
    pub index_drain: Arc<IndexDrain>,
    /// Which runtime pipeline legs are live (dense / rerank / extraction).
    pub pipeline: PipelineStatus,
    /// Wakes the background index sweeper after a write (non-blocking nudge).
    pub(crate) index_wake: Arc<(std::sync::Mutex<bool>, Condvar)>,
    /// Whether a `consolidate` trigger joins its wave instead of queueing it.
    ///
    /// False is the long-lived-binding default: the caller gets `accepted` and
    /// the wave runs in the background. True is for a process that exits as
    /// soon as the command returns, where a detached wave would be lost.
    /// It changes when the work is observed, not what the work is.
    pub(crate) join_consolidate_wave: bool,
    /// Automatic maintenance-trigger decision state, keyed by bank.
    ///
    /// The background sweeper's trigger evaluation (`maintenance_trigger`)
    /// writes it; the status surfaces read it non-blockingly. Behind an `Arc`
    /// so every clone of the state reads one record.
    pub maintenance_trigger: Arc<MaintenanceTriggerState>,
    /// Sync journal / cursors / conflicts / DLQ view of the same backend
    /// (multi-host sync surfaces).
    pub sync: Arc<dyn SyncStore>,
    /// Local DEK provider shared with the store; sync serve seals journaled
    /// content with the same keys the local apply path uses.
    pub kms: Arc<dyn DekProvider>,
    /// This node's sync device id. Default: `device-{pid}` (per-process
    /// identity; `McpOpenOptions` carries no device field).
    pub device_id: String,
    /// Bank-scoped retention profiles (verbosity and duplicate-tolerance policy).
    ///
    /// Behind an `Arc` so every per-request clone locks and writes the same store.
    pub retention: Arc<std::sync::Mutex<RetentionStore>>,
    /// Effective-config `Runtime` behind the config/ranking tools.
    ///
    /// `McpState::open` keeps the env-aware runtime it already resolved
    /// providers from, so `config_get` answers with the same document the
    /// state was built against. `open_with_effective` hosts get a hermetic,
    /// env-free runtime: the provider leg still uses the caller's document,
    /// while the config tools serve that runtime's own (defaults-only) view.
    ///
    /// Behind an `Arc` so a per-request clone serves one runtime; a second
    /// `Runtime` would answer `config_get` with a different document.
    pub config: Arc<std::sync::Mutex<clio_config::Runtime>>,
    /// Durable retention store file, when one is configured for this process.
    pub retention_path: Option<std::path::PathBuf>,
    /// Owner-only erasure-journal path a host-supplied document named (see
    /// `erasure_journal_path_from_document`).
    ///
    /// `Some` comes only from [`McpState::open_with_effective`], the
    /// constructor for a host that already resolved its own configuration:
    /// the erase boundary records the denial there and no ambient environment
    /// is read. Every other constructor — `open`, `open_one_shot`, and
    /// `open_with_runtime` — leaves this `None`, so the erase boundary keeps
    /// the deployment resolution it has always applied: the
    /// `CLIO_ERASURE_JOURNAL_FILE` override over the resolved data directory.
    /// Internal configuration, never a caller or MCP argument.
    pub journal_path: Option<std::path::PathBuf>,
    /// Monotonic counter for caller-omitted item ids (see `runtime_context`).
    ///
    /// Behind an `Arc` because the counter must stay process-wide: a per-request
    /// clone starting from the same value would mint colliding item ids.
    pub(crate) id_counter: Arc<std::sync::atomic::AtomicU64>,
    /// The credential-bound identity for calls dispatched against this state.
    ///
    /// `None` is the unbound contract: the caller-declared actor wins, and any
    /// bank except the deployment-disabled `shared` one is reachable. That is
    /// what the local transports use, because a local process has no credential
    /// edge — the process owner already holds the store.
    pub(crate) principal: Option<Arc<Principal>>,
    /// A pinned clock for the read handlers, set by tests that compare the
    /// protocol handler with direct dispatch. `None` (every production build,
    /// because the field is `cfg(test)`) keeps the engine's system-clock
    /// fallback. Test-only.
    #[cfg(test)]
    pub(crate) test_now: Option<String>,
}

impl std::fmt::Debug for McpState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpState")
            .field("default_bank", &self.default_bank)
            .field("default_actor", &self.default_actor)
            .finish_non_exhaustive()
    }
}

/// Which optional runtime pipeline legs resolved from the effective config.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PipelineStatus {
    /// An embedder is attached to retrieval and the index drain's dense leg.
    pub dense_available: bool,
    /// A configured reranker is attached to retrieval.
    pub rerank_active: bool,
    /// An extraction endpoint is configured for raw ingest.
    pub extract_available: bool,
}

/// Options for [`McpState::open`].
#[derive(Debug, Clone)]
pub struct McpOpenOptions {
    /// `sqlite` or `postgres`.
    pub backend: String,
    /// Postgres URL, or SQLite `:memory:` / filesystem path.
    pub database_url: String,
    /// Bank used when tool args omit one.
    pub default_bank: String,
    /// Default actor attribution.
    pub default_actor: Actor,
    /// Enable the cross-agent shared bank surface (`shared_*` tools).
    pub shared_bank_enabled: bool,
    /// Optional local dev-KMS key-store path. `None` keeps DEKs process-local,
    /// which is correct for an in-memory database and for tests. A host that
    /// needs keys to survive the process sets this (the CLI resolves it through
    /// `clio_config::kms_path::resolve_kms_path`), so a one-shot `remember` and
    /// a later `recall` process share the same DEKs.
    pub kms_path: Option<std::path::PathBuf>,
}

impl Default for McpOpenOptions {
    fn default() -> Self {
        Self {
            backend: "sqlite".into(),
            database_url: "sqlite::memory:".into(),
            default_bank: "default".into(),
            default_actor: Actor::Agent,
            shared_bank_enabled: false,
            kms_path: None,
        }
    }
}

/// Effective-document key naming the owner-only erasure journal.
///
/// The key is the injection seam for a host that already resolved its own
/// configuration and opens through [`McpState::open_with_effective`]; it
/// mirrors `clio_config::ERASURE_JOURNAL_FILE_ENV`, which stays the deployment
/// override. The deployment constructors (`open`, `open_one_shot`) never read
/// it, so they keep resolving the journal exactly as they always have.
pub(crate) const ERASURE_JOURNAL_FILE_POINTER: &str = "/erasure_journal_file";

/// The owner-only erasure-journal path an effective document names, if any.
///
/// A blank or NUL-bearing value counts as unnamed, matching the rule
/// `clio_config::resolve_journal_path` applies to its own override.
pub(crate) fn erasure_journal_path_from_document(effective: &Value) -> Option<std::path::PathBuf> {
    effective
        .pointer(ERASURE_JOURNAL_FILE_POINTER)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|path| !path.is_empty() && !path.contains('\0'))
        .map(std::path::PathBuf::from)
}

impl McpState {
    /// A shallow copy of this state carrying `principal`, for one request.
    ///
    /// Every shared field stays one instance because each is already behind an
    /// `Arc`; the copy differs only in who it acts as. The credential is behind
    /// an `Arc` too, so a request that carries a list of banks does not copy it.
    #[must_use]
    pub(crate) fn with_principal(&self, principal: Principal) -> McpState {
        let mut scoped = self.clone();
        scoped.principal = Some(Arc::new(principal));
        scoped
    }

    /// Refuse an administrative mutation when the bound credential is not an
    /// administrator.
    ///
    /// A credential names one actor, and only `harness` and `system` carry the
    /// privilege the config, ranking, and deployment-retention mutations need —
    /// the same administrative set the compliance verbs use. An unbound state
    /// (a local transport with no credential edge) is allowed, because the
    /// process owner already holds the store.
    ///
    /// # Errors
    /// [`ErrorCode::Forbidden`] when a bound credential's actor is not an
    /// administrator.
    pub(crate) fn authorize_admin(&self) -> Result<(), AmError> {
        let Some(principal) = &self.principal else {
            return Ok(());
        };
        match principal.actor {
            Actor::Harness | Actor::System => Ok(()),
            Actor::Agent | Actor::User => Err(AmError::new(
                ErrorCode::Forbidden,
                "this credential is not an administrator; config, ranking, \
                 and deployment-retention changes need a harness or system \
                 credential",
            )),
        }
    }

    /// Open MCP runtime state over one backend.
    ///
    /// Providers (embed / rerank / extract) are resolved from the effective
    /// configuration, which already layers the process environment, so the
    /// config a setup wizard writes takes effect without extra env vars.
    ///
    /// # Errors
    /// Unknown backend, store-open failures, or a partially configured provider
    /// (fail closed with a clear message instead of silently disabling a service).
    pub fn open(options: &McpOpenOptions) -> Result<Arc<Self>, AmError> {
        let runtime = clio_config::Runtime::new();
        Self::open_with_runtime(options, runtime)
    }

    /// Open runtime state for a process that exits as soon as its command
    /// returns, so each `consolidate` wave completes before the call returns.
    ///
    /// Behavior is identical to [`Self::open`] except that `consolidate` runs
    /// its wave on the calling thread and reports the real result, instead of
    /// queueing it on a thread that dies with the process. The MCP server keeps
    /// the asynchronous `accepted` response; use [`Self::open`] there.
    ///
    /// # Errors
    /// As [`Self::open`].
    pub fn open_one_shot(options: &McpOpenOptions) -> Result<Arc<Self>, AmError> {
        let runtime = clio_config::Runtime::new();
        let effective = runtime.effective_raw();
        // Not the document seam: a deployment process keeps the
        // environment-over-data-directory journal resolution.
        Self::open_with_runtime_effective_and_wave(options, runtime, &effective, true, None)
    }

    /// Open runtime state against an explicit effective config document.
    ///
    /// Hosts and tests that already resolved configuration use this to avoid a
    /// second environment read; behavior is identical to [`Self::open`] except
    /// that the config/ranking tools are served by a hermetic, env-free
    /// runtime (defaults only), while providers resolve from the caller's
    /// document. This is also the one constructor that honours a
    /// document-named erasure journal (`/erasure_journal_file`), because a
    /// host that resolved the document is naming the path; the deployment
    /// constructors never read that key.
    ///
    /// # Errors
    /// As [`Self::open`].
    pub fn open_with_effective(
        options: &McpOpenOptions,
        effective: &Value,
    ) -> Result<Arc<Self>, AmError> {
        Self::open_with_runtime_and_effective(
            options,
            clio_config::Runtime::with_env(&|_| None),
            effective,
            erasure_journal_path_from_document(effective),
        )
    }

    /// Open runtime state keeping `runtime` as the effective-config `Runtime`
    /// served by the config/ranking tools.
    ///
    /// Unlike [`Self::open_with_effective`], a document-named erasure journal
    /// is not taken: the path stays unset and the erase boundary keeps its
    /// environment-over-data-directory resolution.
    ///
    /// # Errors
    /// As [`Self::open`].
    pub fn open_with_runtime(
        options: &McpOpenOptions,
        runtime: clio_config::Runtime,
    ) -> Result<Arc<Self>, AmError> {
        let effective = runtime.effective_raw();
        Self::open_with_runtime_and_effective(options, runtime, &effective, None)
    }
}

pub use crate::runtime_context::now_utc;
pub(crate) use crate::runtime_open::extract_available_from;
pub use crate::runtime_open::load_retention_store;

#[cfg(test)]
#[path = "runtime_tests.rs"]
mod runtime_tests;

#[cfg(test)]
#[path = "runtime_sink_tests.rs"]
mod runtime_sink_tests;

#[cfg(test)]
#[path = "runtime_stamp_tests.rs"]
mod runtime_stamp_tests;
