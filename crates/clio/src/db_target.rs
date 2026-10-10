// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the one place a CLI command turns its flags
//! and environment into the database target it will open.
//!
//! ## Owns
//! - Building the runtime whose effective document decides the target
//! - Wrapping `clio-config`'s effective-document resolver for every command
//!
//! ## Does not own
//! - The precedence chain itself (`clio-config::db_effective` owns that)
//! - Opening the store (`clio-mcp` and `clio-store` own that)
//! - Which flag a command accepts (`cli_args` owns the grammar)
//!
//! ## Boundary
//!
//! Changes to how a command finds its store should normally require changes
//! here. Changes to the precedence chain should not require changes here.
//!
//! Keep this module focused on resolving one target for one command.

use clio_config::db_path::ResolvedDatabase;
use clio_config::{Runtime, db_effective::resolve_effective_database};

/// The database target one command will open, from its flags and the config.
///
/// The runtime is built here rather than inside the resolver because the
/// resolver reads a document and only a runtime can assemble one: the config
/// file, the process environment, the active profile, and the deployment
/// overlay. A command that skipped this used to open whatever the environment
/// chain named, so a value `clio setup` wrote into the deployment config could
/// never change which store it opened.
///
/// `get` is a trait object so a command can pass its own injected lookup and a
/// test never reads the host environment.
///
/// # Errors
/// The configured address or backend names something no store can open. The
/// reason is carried through so the command can print it instead of opening a
/// local SQLite file the operator never asked for.
pub(crate) fn resolve_target(
    get: &dyn Fn(&str) -> Option<String>,
    explicit_db: Option<&str>,
    explicit_backend: Option<&str>,
) -> Result<ResolvedDatabase, String> {
    let lookup = |key: &str| get(key);
    let runtime = Runtime::with_env(&lookup);
    resolve_effective_database(
        &lookup,
        &runtime.effective_raw(),
        explicit_db,
        explicit_backend,
    )
}

#[cfg(test)]
#[path = "db_target_tests.rs"]
mod tests;
