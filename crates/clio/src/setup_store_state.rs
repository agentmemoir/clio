// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the one read the setup switch gate needs:
//! what the store Clio currently points at holds, counted through the normal
//! store opener.
//!
//! ## Owns
//! - `current`: the current effective target, compared with the planned one
//! - `open_counts`: the read-only open of the current store and its two counts
//! - The "same target is not a change" and "a missing file is an error" rules
//!
//! ## Does not own
//! - The typed `switch` prompt and the warning it prints (see `setup_report_amend`)
//! - Resolving a target from flags and environment (`db_target` owns that)
//! - Opening a store for any other command (`status_cli` owns that)
//!
//! ## Boundary
//!
//! Changes to what the switch gate counts, or to when it counts, should normally
//! require changes here. Changes to what the operator reads, or to when a
//! confirmation is required, should not.
//!
//! Keep this module focused on counting the current store.
//!
//! # Why a missing SQLite file is an error and not an empty store
//!
//! `SqliteStore::open` creates the database and runs migrations, so using it on
//! a missing file would create the very store the gate is deciding not to read,
//! and the count would then be a fact about the wizard rather than about the
//! operator's data. The path is checked first; a file that is not there is an
//! unreadable store, which still requires the typed confirmation.
//!
//! # Why an unresolved target is unreadable rather than empty
//!
//! The target comes from the effective document. When that document cannot
//! resolve, Clio does not know which store it has been using: claiming "empty"
//! would skip the one confirmation that protects data, so the reason is carried
//! out and the gate stays closed.

use crate::setup_review::StoreState;
use clio_config::db_path::ResolvedDatabase;
use clio_store::{DekProvider, LocalDevKms, OpsStore, PostgresStore, StoreCounts};
use std::path::Path;
use std::sync::Arc;

/// What the store Clio currently points at holds, for the switch gate.
///
/// A run that configures the target it already has, or configures anything at
/// all when no config exists yet, changes nothing and reports [`StoreState::Empty`].
/// A different target is opened read-only and counted.
pub(crate) fn current(
    env: &dyn Fn(&str) -> Option<String>,
    config_path: &Path,
    planned_backend: &str,
    planned_url: &str,
) -> StoreState {
    // No config file means there is no store this run could be leaving behind.
    // The default target a fresh machine resolves is not one Clio has ever
    // written, so counting it would invent a store to warn about.
    if !config_path.exists() {
        return StoreState::Empty;
    }
    let resolved = match crate::db_target::resolve_target(env, None, None) {
        Ok(resolved) => resolved,
        Err(reason) => return StoreState::Unreadable { reason },
    };
    if resolved.backend == planned_backend && resolved.url == planned_url {
        return StoreState::Empty;
    }
    match open_counts(&resolved) {
        Ok(counts) if counts.memories == 0 && counts.subjects == 0 => StoreState::Empty,
        Ok(counts) => StoreState::NonEmpty {
            memories: counts.memories,
            subjects: counts.subjects,
        },
        Err(reason) => StoreState::Unreadable { reason },
    }
}

/// Open the resolved target read-only and count what is in it.
///
/// The guards mirror `status_cli::open_chosen`: a missing SQLite file is an
/// error rather than a file to create, and an unknown backend is refused. The
/// counts themselves come from the store's own operator contract, so both
/// backends answer the same question the same way.
///
/// A SQLite file is counted through `clio_store::sqlite_store_counts`, which
/// opens it `SQLITE_OPEN_READ_ONLY`: the normal opener runs migrations and
/// updates the file, and the plan's gate test asserts the old store is
/// byte-identical after a successful switch. A Postgres target is opened
/// through its normal opener, which is what `status` does too.
fn open_counts(resolved: &ResolvedDatabase) -> Result<StoreCounts, String> {
    match resolved.backend.as_str() {
        "sqlite" => {
            if resolved.url == "sqlite::memory:" {
                // An in-memory store persists nothing, so there is nothing to
                // leave behind.
                return Ok(StoreCounts::default());
            }
            let path = Path::new(&resolved.url);
            if !path.exists() {
                return Err(format!("database file does not exist: {}", resolved.url));
            }
            clio_store::sqlite_store_counts(path).map_err(|e| e.message)
        }
        "postgres" => {
            let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
            let store = PostgresStore::open(&resolved.url, kms).map_err(|e| e.message)?;
            store.counts().map_err(|e| e.message)
        }
        other => Err(format!("backend must be sqlite|postgres, got `{other}`")),
    }
}

#[cfg(test)]
#[path = "setup_store_state_tests.rs"]
mod tests;
