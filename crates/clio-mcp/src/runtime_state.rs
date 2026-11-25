// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the shape of the shared MCP runtime state: the
//! `McpState` field set every transport hands to the tool dispatchers, and the
//! one refusal a listener that serves with no credential off loopback answers a
//! privileged caller-declared actor with.
//!
//! ## Owns
//! - The `McpState` struct definition: its fields, and the invariant each one
//!   carries (shared-behind-an-`Arc`, per-request, or deployment fact)
//! - [`UNAUTHENTICATED_TRANSPORT_REFUSAL`]: the one message the actor and
//!   administrative gates refuse with when the transport cannot vouch for a
//!   caller-declared privileged actor
//!
//! ## Does not own
//! - The open API, the open-time gates, and the setters over this state (see
//!   `runtime`)
//! - Constructing one over a store backend (see `runtime_open`)
//! - Per-call actor and bank resolution (see `runtime_context`)
//!
//! ## Boundary
//!
//! Changes to which fields the state carries should normally require changes
//! here, and in every literal that builds one (see `runtime_open`). Changes to
//! what a transport does with the state should not.
//!
//! Keep this module to the state's shape: it holds no function body, so it
//! compiles to no executable line of its own.

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
use clio_types::Actor;
use clio_write::{HubDistillEngine, LeafIndex, MemForest, MemTreeMaint};

use crate::index_drain::IndexDrain;
use crate::maintenance_trigger::MaintenanceTriggerState;
use crate::principal::Principal;
use crate::runtime::PipelineStatus;
use std::sync::{Arc, Condvar};

/// The one refusal a listener that serves with no credential outside loopback
/// answers a privileged caller-declared actor with.
///
/// An unbound state normally lets the caller declare the actor, because a local
/// process owner already holds the store. A listener reachable off this machine
/// has no credential to vouch for that declaration, so it refuses `harness` and
/// `system` there. The text names the transport rule and nothing else: no
/// secret, no host, and no path.
pub(crate) const UNAUTHENTICATED_TRANSPORT_REFUSAL: &str = "this listener is unauthenticated and reachable off this machine, so it does not honour a \
     caller-declared harness or system actor";

/// Shared state behind every MCP transport: one store, one engine set.
///
/// `Clone` is a shallow copy, and every field that must stay single and
/// unforked sits behind an `Arc`: the store engines, the index drain, the
/// retention store, and the config runtime. A clone therefore shares one
/// instance of each and can differ only in [`Self::principal`], which is what
/// makes a per-request principal cheap and safe to attach.
#[derive(Clone)]
#[expect(clippy::struct_excessive_bools, reason = "deployment flags")]
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
    /// Live-store facts, measured once at open; a failed measurement fails the
    /// open, and only an in-memory store opens with no measurement.
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
    pub(crate) tool_signals: Arc<crate::tool_signals::ToolSignals>,
    /// Whether a `consolidate` trigger joins its wave instead of queueing it.
    ///
    /// False is the long-lived-binding default: the caller gets `accepted` and
    /// the wave runs in the background. True is for a process that exits as
    /// soon as the command returns, where a detached wave would be lost.
    /// It changes when the work is observed, not what the work is.
    pub(crate) join_consolidate_wave: bool,
    /// Automatic maintenance-trigger decision state, keyed by bank: the
    /// background sweeper writes it and the status surfaces read it
    /// non-blockingly. Behind an `Arc` so every clone reads one record.
    pub maintenance_trigger: Arc<MaintenanceTriggerState>,
    /// Sync journal / cursors / conflicts / DLQ view of the same backend
    /// (multi-host sync surfaces).
    pub sync: Arc<dyn SyncStore>,
    /// Local DEK provider shared with the store; sync serve seals journaled
    /// content with the same keys the local apply path uses.
    pub kms: Arc<dyn DekProvider>,
    /// This node's durable sync device id: read from the installation's store
    /// and minted on first open, overridden by `CLIO_SYNC_DEVICE_ID` when set.
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
    /// env-free runtime whose base layer is the document they supplied.
    ///
    /// Behind an `Arc` so a per-request clone serves one runtime, not a second document.
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
    /// Deployment-owned root every `export` / `review_export` destination must
    /// resolve inside, from the export-root precedence chain; never created here.
    pub export_root: std::path::PathBuf,
    /// Whether `export_root` is deployment-owned rather than the relative
    /// fallback tier resolved against a client project's working directory.
    pub export_root_trusted: bool,
    /// Monotonic counter for caller-omitted item ids (see `runtime_context`).
    ///
    /// Behind an `Arc` because the counter must stay process-wide: a per-request
    /// clone starting from the same value would mint colliding item ids.
    pub(crate) id_counter: Arc<std::sync::atomic::AtomicU64>,
    pub(crate) calls: Arc<crate::call_accounting::DirectCallAccounting>,
    /// The credential-bound identity for calls dispatched against this state.
    ///
    /// `None` is the unbound contract: the caller-declared actor wins, and any
    /// bank except the deployment-disabled `shared` one is reachable. That is
    /// what the local transports use, because a local process has no credential
    /// edge — the process owner already holds the store.
    pub(crate) principal: Option<Arc<Principal>>,
    /// Whether this state is served over a listener that cannot vouch for a
    /// caller-declared actor: an unbound one reachable off this machine.
    ///
    /// Only the HTTP transport sets it, and only for a bind outside loopback
    /// with no credential source. A local process and a loopback listener keep
    /// the unbound contract above, because their owner already holds the store;
    /// a listener every interface can reach may not be told by its caller that
    /// it acts as `harness` or `system`.
    pub(crate) network_untrusted: bool,
    /// A pinned clock for the read handlers, set by tests that compare the
    /// protocol handler with direct dispatch. `None` (every production build,
    /// because the field is `cfg(test)`) keeps the engine's system-clock
    /// fallback. Test-only.
    #[cfg(test)]
    pub(crate) test_now: Option<String>,
}
