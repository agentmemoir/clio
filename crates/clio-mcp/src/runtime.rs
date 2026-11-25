// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the MCP runtime state's open API and the gates
//! that run over it: the entry points the transports hand to the tool
//! dispatchers.
//!
//! ## Owns
//! - The public `open` entry points and their effective-config resolution policy
//!   (`open` vs `open_with_effective`), including the document-named
//!   erasure-journal seam
//! - Bank/actor defaults, the shared-bank permission gate, the
//!   administrative-mutation gate, and the transport-trust gate
//! - The setters a host or a transport needs over an opened state
//! - stderr telemetry sink adapters (events never touch stdout)
//!
//! ## Does not own
//! - The `McpState` struct and its field set (see `runtime_state`)
//! - Store-backend construction and engine wiring (see `runtime_open`)
//! - Tool dispatch logic (see `write_tools`)
//! - Admission, storage, or write-path semantics
//! - Per-call context/id/timestamp helpers (see `runtime_context`)
//! - Index-drain scheduling and status (see `runtime_index` / `index_drain`)
//!
//! ## Boundary
//!
//! Changes to the `McpState` field set should normally require changes in
//! `runtime_state`, and changes to its open API or a gate over it here. Changes
//! to which engines a store is wired with should not require changes here.
//!
//! Keep this module focused on the MCP runtime state's open API and its gates.

use clio_types::{Actor, AmError, ErrorCode};

pub use crate::ops_embedder::{build_ops_embedder, build_ops_embedder_with};
use crate::principal::Principal;
use serde_json::Value;
use std::sync::Arc;

#[path = "runtime_state.rs"]
mod runtime_state;

pub use self::runtime_state::McpState;
pub(crate) use self::runtime_state::UNAUTHENTICATED_TRANSPORT_REFUSAL;
pub use crate::runtime_sinks::{BatchSignals, NoSignals, StderrSinks, StoreSignals};

/// Pinned protocol revision carried for diagnostics assertions.
pub const PROTOCOL_REVISION: &str = crate::MCP_PROTOCOL_REVISION;

/// Dedicated cross-agent shared bank namespace. Access is gated at
/// bank resolution so no tool can route around the deployment permission.
pub const SHARED_BANK: &str = "shared";

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
    /// privilege the config, ranking, deployment-retention, and bundle import
    /// mutations need — the same administrative set the compliance verbs use.
    /// An unbound state is allowed, because the process owner already holds the
    /// store, unless the listener is the unauthenticated off-machine one: there
    /// nobody's declaration of an administrative actor is worth anything, so the
    /// unbound allowance does not reach it.
    ///
    /// # Errors
    /// [`ErrorCode::Forbidden`] when a bound credential's actor is not an
    /// administrator, or when the state is unbound and served off this machine
    /// without a credential.
    pub(crate) fn authorize_admin(&self) -> Result<(), AmError> {
        let Some(principal) = &self.principal else {
            return self.require_authenticated_transport();
        };
        if principal.actor.is_privileged() {
            return Ok(());
        }
        Err(AmError::new(
            ErrorCode::Forbidden,
            "this credential is not an administrator; config, ranking, \
             deployment-retention, and bundle import changes need a \
             harness or system credential",
        ))
    }

    /// Refuse when this state is served by a listener that cannot vouch for a
    /// caller-declared actor.
    ///
    /// Only an unbound state is refused: a bound one names its actor from the
    /// credential, so the transport does not have to be trusted for that.
    ///
    /// # Errors
    /// [`ErrorCode::Forbidden`] on an unauthenticated off-machine listener.
    pub(crate) fn require_authenticated_transport(&self) -> Result<(), AmError> {
        if self.principal.is_none() && self.network_untrusted {
            return Err(AmError::new(
                ErrorCode::Forbidden,
                UNAUTHENTICATED_TRANSPORT_REFUSAL,
            ));
        }
        Ok(())
    }

    /// Mark this state as served by a listener reachable off this machine.
    ///
    /// Called by the HTTP transport on the per-request copy (see
    /// `http_post::scoped_state`), never on a state a local transport shares.
    pub(crate) fn mark_network_untrusted(&mut self) {
        self.network_untrusted = true;
    }

    /// Override the export root with an explicitly supplied, deployment-owned
    /// path (embedders and tests); the first confined destination creates it.
    pub fn set_export_root(&mut self, path: impl Into<std::path::PathBuf>) {
        self.export_root = path.into();
        self.export_root_trusted = true;
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
    /// that the config/ranking tools are served by a hermetic runtime that reads
    /// no environment and takes the caller's document as its base layer, so a
    /// tool profile the document selects governs the listing. This is also the
    /// one constructor that honours a document-named erasure journal
    /// (`/erasure_journal_file`), because a host that resolved the document is
    /// naming the path; the deployment constructors never read that key.
    ///
    /// # Errors
    /// As [`Self::open`].
    pub fn open_with_effective(
        options: &McpOpenOptions,
        effective: &Value,
    ) -> Result<Arc<Self>, AmError> {
        let mut runtime = clio_config::Runtime::with_env(&|_| None);
        runtime.load_file_json(&effective.to_string())?;
        Self::open_with_runtime_and_effective(
            options,
            runtime,
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
pub use crate::runtime_open::extract_available_from;
pub use crate::runtime_open::load_retention_store;

#[cfg(test)]
#[path = "runtime_tests.rs"]
mod runtime_tests;

#[cfg(test)]
#[path = "runtime_transport_tests.rs"]
mod runtime_transport_tests;

#[cfg(test)]
#[path = "runtime_sink_tests.rs"]
mod runtime_sink_tests;

#[cfg(test)]
#[path = "runtime_stamp_tests.rs"]
mod runtime_stamp_tests;
