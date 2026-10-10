// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests that a command's database target comes from the config
//! file, not only from the environment.
//!
//! ## Owns
//! - The silent-SQLite regression at the command layer: a deployment config
//!   that names a Postgres address decides the target
//! - The fallbacks a command takes when no config names a store
//!
//! ## Does not own
//! - The precedence chain itself (see `clio_config::db_effective`)
//! - Flag parsing (`cli_args`)
//!
//! ## Boundary
//!
//! Tests here break when a command stops consulting the effective config; they
//! never change the implementation.
//!
//! Keep this module focused on how a command finds its store.

use super::*;
use crate::setup_support_tests::{env_at, temp_dir};
use serde_json::json;

/// Write a deployment config through the save path, resolve a target through
/// it, and clean up. The address sits at a secret path, so a hand-written
/// plaintext file would be refused by the loader; sealing through `config_set`
/// is what `clio setup` does.
fn resolve_through(url: &str) -> Result<ResolvedDatabase, String> {
    let dir = temp_dir("db-target");
    let path = dir.join("deployment.json");
    let mut seed = Runtime::with_deployment_file(&path);
    let clio_config::error::ToolResult::Ok { .. } = seed.config_set(
        "store.database_url",
        json!(url),
        clio_config::ConfigScope::Deployment,
    ) else {
        panic!("config_set store.database_url");
    };
    let get = env_at(&path);
    let resolved = resolve_target(&get, None, None);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_dir(&dir);
    resolved
}

/// The regression this stage exists for. The deployment config names a
/// Postgres address, so the target is Postgres and that address, whatever the
/// seeded `backend: sqlite` default says. Before the effective-document
/// resolver, every command read the environment chain alone and opened the
/// default SQLite file, which is how `clio status` could report Postgres while
/// `clio remember` wrote to SQLite.
#[test]
fn a_configured_postgres_address_decides_a_commands_target() {
    let resolved =
        resolve_through("postgres://configured/db").expect("a configured address is openable");
    assert_eq!(resolved.backend, "postgres");
    assert_eq!(resolved.url, "postgres://configured/db");
}

/// With no config and no environment, a command still falls back to the
/// SQLite file under the data directory rather than failing.
#[test]
fn an_empty_environment_falls_back_to_a_relative_sqlite_file() {
    let get = |_key: &str| None;
    let resolved = resolve_target(&get, None, None).expect("resolves");
    assert_eq!(resolved.backend, "sqlite");
    assert_eq!(resolved.url, "clio.db");
}

/// A malformed configured address is reported, not turned into a local file.
#[test]
fn a_malformed_configured_address_is_refused() {
    let error =
        resolve_through("mysql://configured/db").expect_err("a mysql server cannot be opened");
    assert!(error.contains("mysql://configured/db"), "{error}");
}
