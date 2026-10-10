// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the setup switch gate's read of the
//! current store: same-target and no-config as empty, real counts as
//! non-empty, and every unreadable state as a closed gate.
//!
//! ## Owns
//! - Current-store state under test for `setup_store_state`
//!
//! ## Does not own
//! - Production counting logic under test
//! - The switch prompt itself (see `setup_report_amend_tests`)
//!
//! ## Boundary
//!
//! Changes to when the gate requires a typed confirmation should normally update
//! tests here. Changes to the prompt's wording should not.
//!
//! Keep this module focused on the store-state decision.

use super::*;
use crate::setup_support_tests::{env_at, seed_sqlite_store, temp_dir};
use serde_json::json;

/// Write a deployment config that names one SQLite target.
///
/// The address sits at a secret path, so the save path must seal it: a
/// hand-written plaintext file would be refused by the loader before the
/// switch gate ever read the store.
fn write_config(path: &Path, backend: &str, url: &str) {
    let mut rt = clio_config::Runtime::with_deployment_file(path);
    for (key, value) in [("backend", backend), ("store.database_url", url)] {
        let clio_config::error::ToolResult::Ok { .. } =
            rt.config_set(key, json!(value), clio_config::ConfigScope::Deployment)
        else {
            panic!("config_set {key}");
        };
    }
}

#[test]
fn no_config_file_means_no_current_store() {
    let dir = temp_dir("state-none");
    let config = dir.join("deployment.json");
    let env = env_at(&config);
    assert_eq!(
        current(
            &env,
            &config,
            "postgres",
            "postgres://clio:pw@db.example.com:5432/clio"
        ),
        StoreState::Empty
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn configuring_the_same_target_is_not_a_change() {
    let dir = temp_dir("state-same");
    let config = dir.join("deployment.json");
    let store = dir.join("memories.db");
    let url = store.to_string_lossy().into_owned();
    write_config(&config, "sqlite", &url);
    seed_sqlite_store(&store, &[("i1", "s1"), ("i2", "s2")]);
    let env = env_at(&config);

    // Rows are present, but the planned target is the one already configured,
    // so nothing is being left behind.
    assert_eq!(current(&env, &config, "sqlite", &url), StoreState::Empty);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_empty_store_on_a_different_target_needs_nothing() {
    let dir = temp_dir("state-empty");
    let config = dir.join("deployment.json");
    let old = dir.join("old.db");
    write_config(&config, "sqlite", &old.to_string_lossy());
    seed_sqlite_store(&old, &[]);
    let env = env_at(&config);
    let new = dir.join("new.db");

    assert_eq!(
        current(&env, &config, "sqlite", &new.to_string_lossy()),
        StoreState::Empty
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn rows_in_the_current_store_are_counted_as_memories_and_subjects() {
    let dir = temp_dir("state-rows");
    let config = dir.join("deployment.json");
    let old = dir.join("old.db");
    write_config(&config, "sqlite", &old.to_string_lossy());
    seed_sqlite_store(&old, &[("i1", "s1"), ("i2", "s1"), ("i3", "s2")]);
    let env = env_at(&config);

    assert_eq!(
        current(
            &env,
            &config,
            "postgres",
            "postgres://clio:pw@db.example.com:5432/clio"
        ),
        StoreState::NonEmpty {
            memories: 3,
            subjects: 2
        }
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_missing_current_store_is_unreadable_not_empty() {
    let dir = temp_dir("state-missing");
    let config = dir.join("deployment.json");
    let missing = dir.join("never-created.db");
    write_config(&config, "sqlite", &missing.to_string_lossy());
    let env = env_at(&config);

    let state = current(
        &env,
        &config,
        "sqlite",
        &dir.join("other.db").to_string_lossy(),
    );
    assert!(
        matches!(state, StoreState::Unreadable { ref reason } if reason.contains("does not exist")),
        "{state:?}"
    );
    assert!(state.consequence().is_some(), "the gate stays closed");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_target_that_cannot_be_resolved_is_unreadable() {
    let dir = temp_dir("state-broken");
    let config = dir.join("deployment.json");
    write_config(&config, "sqlite", "mysql://db.example.com/clio");
    let env = env_at(&config);

    assert!(
        matches!(current(&env, &config, "sqlite", "/tmp/other.db"), StoreState::Unreadable { ref reason } if reason.contains("postgres://")),
        "a hand-edited address must close the gate"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
