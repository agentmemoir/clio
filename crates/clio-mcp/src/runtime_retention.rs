// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the MCP runtime's bank-scoped retention profile
//! accessors: effective resolution, the admission snapshot, and durable writes.
//!
//! ## Owns
//! - `retention_effective` / `resolved_retention` / `set_retention` on `McpState`
//! - The retention store path override used by embedders and tests
//!
//! ## Does not own
//! - `McpState` construction and field wiring (see `runtime`)
//! - Profile tool argument decoding (see `retention_tools`)
//!
//! ## Boundary
//!
//! Changes to how the runtime resolves or persists bank profiles should normally
//! require changes here. Changes to profile tool handlers should not require
//! changes here.
//!
//! Keep this module focused on runtime retention profile access.

use crate::runtime::McpState;
use clio_config::retention::{ResolvedRetention, RetentionProfile};
use clio_config::retention_store::{ProfileSource, RetentionStore};
use clio_types::AmError;

impl McpState {
    /// Effective retention profile for a bank plus where it came from.
    #[must_use]
    pub fn retention_effective(&self, bank: &str) -> (RetentionProfile, ProfileSource) {
        self.retention
            .lock()
            .expect("retention store poisoned")
            .effective(bank)
    }

    /// Admission snapshot for a bank, ready to hand to the admission policy.
    #[must_use]
    pub fn resolved_retention(&self, bank: &str) -> ResolvedRetention {
        self.retention_effective(bank).0.resolve()
    }

    /// Store a validated bank override, persisting it when a path is configured.
    ///
    /// Returns whether the profile was written to disk. A failed persist rolls
    /// the in-memory store back, so the caller-visible state never holds the change.
    ///
    /// # Errors
    /// An empty bank name, an invalid profile, or a persistence failure.
    pub fn set_retention(&self, bank: &str, profile: RetentionProfile) -> Result<bool, AmError> {
        let mut store = self.retention.lock().expect("retention store poisoned");
        let previous = store.bank_override(bank).cloned();
        store.set_bank(bank, profile)?;
        match persist_locked(self.retention_path.as_deref(), &store) {
            Ok(persisted) => Ok(persisted),
            Err(e) => {
                restore_override(&mut store, bank, previous);
                Err(e)
            }
        }
    }

    /// Store a validated deployment default, persisting it when a path is configured.
    ///
    /// A failed persist rolls the in-memory store back, so the caller-visible
    /// state never holds the change.
    ///
    /// # Errors
    /// An invalid profile or a persistence failure.
    pub fn set_retention_deployment(&self, profile: RetentionProfile) -> Result<bool, AmError> {
        let mut store = self.retention.lock().expect("retention store poisoned");
        let previous = store.deployment.clone();
        store.set_deployment(profile)?;
        match persist_locked(self.retention_path.as_deref(), &store) {
            Ok(persisted) => Ok(persisted),
            Err(e) => {
                store.deployment = previous;
                Err(e)
            }
        }
    }

    /// Override the durable retention store path (embedders and tests).
    ///
    /// An unreadable or corrupt store file degrades to the builtin-only store.
    pub fn set_retention_path(&mut self, path: impl Into<std::path::PathBuf>) {
        let path = path.into();
        let store = RetentionStore::load_lossy(&path);
        self.retention_path = Some(path);
        *self.retention.lock().expect("retention store poisoned") = store;
    }
}

/// Persist the store when a path is configured; `Ok(false)` when in-memory only.
fn persist_locked(path: Option<&std::path::Path>, store: &RetentionStore) -> Result<bool, AmError> {
    match path {
        Some(path) => {
            store.save(path)?;
            Ok(true)
        }
        None => Ok(false),
    }
}

/// Roll the in-memory bank override back so a failed persist leaves no change.
fn restore_override(store: &mut RetentionStore, bank: &str, previous: Option<RetentionProfile>) {
    match previous {
        Some(profile) => store
            .set_bank(bank, profile)
            .expect("rollback profile was validated"),
        None => {
            store.clear_bank(bank);
        }
    }
}

#[cfg(test)]
#[path = "runtime_retention_tests.rs"]
mod runtime_retention_tests;

#[cfg(test)]
#[path = "retention_test_support.rs"]
pub(crate) mod retention_test_support;
