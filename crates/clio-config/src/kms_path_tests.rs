// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for unit-testing the local dev-KMS path resolver:
//! the memory short-circuit, the `CLIO_KMS_FILE` override, and the per-backend
//! fallback shapes.
//!
//! ## Owns
//! - Each precedence tier and its first-match-wins ordering
//! - The SQLite-sibling and Postgres data-dir fallback shapes
//!
//! ## Does not own
//! - The resolver implementation (see `kms_path`)
//! - Key-store reading/writing (`clio-store`)
//!
//! ## Boundary
//!
//! Tests here break when the key-store path shape changes; they never change the
//! implementation.
//!
//! Keep this module focused on resolver unit behavior.

use super::*;

/// Build an injected environment lookup from key/value pairs.
fn env(pairs: &[(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> + 'static {
    let pairs: Vec<(&'static str, &'static str)> = pairs.to_vec();
    move |key: &str| {
        pairs
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| (*v).to_owned())
    }
}

fn none(_: &str) -> Option<String> {
    None
}

#[test]
fn memory_target_has_no_persistent_key_store() {
    assert_eq!(resolve_kms_path(&none, "sqlite::memory:"), None);
}

#[test]
fn empty_target_has_no_persistent_key_store() {
    assert_eq!(resolve_kms_path(&none, "   "), None);
}

#[test]
fn explicit_env_path_wins_for_sqlite_and_postgres() {
    let get = env(&[("CLIO_KMS_FILE", "/keys/clio.kms.json")]);
    assert_eq!(
        resolve_kms_path(&get, "./data/clio.db"),
        Some(PathBuf::from("/keys/clio.kms.json"))
    );
    assert_eq!(
        resolve_kms_path(&get, "postgres://u:p@h/db"),
        Some(PathBuf::from("/keys/clio.kms.json"))
    );
}

#[test]
fn sqlite_file_uses_a_sibling_key_store() {
    assert_eq!(
        resolve_kms_path(&none, "/data/clio.db"),
        Some(PathBuf::from("/data/clio.db.kms.json"))
    );
}

#[test]
fn sqlite_relative_path_stays_relative() {
    assert_eq!(
        resolve_kms_path(&none, "clio.db"),
        Some(PathBuf::from("clio.db.kms.json"))
    );
}

#[test]
fn postgres_falls_back_to_the_data_directory() {
    let get = env(&[("CLIO_DATA_DIR", "/data")]);
    assert_eq!(
        resolve_kms_path(&get, "postgres://u:p@h/db"),
        Some(PathBuf::from("/data/clio.kms.json"))
    );
    let home = env(&[("HOME", "/home/u")]);
    assert_eq!(
        resolve_kms_path(&home, "postgres://u:p@h/db"),
        Some(PathBuf::from("/home/u/.local/share/clio/clio.kms.json"))
    );
}

#[test]
fn postgres_relative_default_stays_relative() {
    assert_eq!(
        resolve_kms_path(&none, "postgresql://u:p@h/db"),
        Some(PathBuf::from("clio.kms.json"))
    );
}

#[test]
fn blank_explicit_env_is_ignored() {
    let get = env(&[("CLIO_KMS_FILE", "   "), ("CLIO_DATA_DIR", "/data")]);
    assert_eq!(
        resolve_kms_path(&get, "postgres://u:p@h/db"),
        Some(PathBuf::from("/data/clio.kms.json"))
    );
}
