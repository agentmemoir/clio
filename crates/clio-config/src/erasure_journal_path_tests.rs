// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for unit-testing erasure-journal path resolution
//! and the deployment identity mint: the override precedence, the data-directory
//! fallback shape, the refusal to follow the process's working directory, and
//! the id's encoding.
//!
//! ## Owns
//! - Every tier of the `CLIO_ERASURE_JOURNAL_FILE` / `<data-dir>` chain
//! - The refusal when no absolute data directory resolves
//! - The 64-character lowercase-hex deployment identity
//!
//! ## Does not own
//! - The resolver and mint implementation (see `erasure_journal_path`)
//! - Journal contents, permissions, and file writes: the module under test
//!   touches no filesystem, so no test here creates a file either
//!
//! ## Boundary
//!
//! Tests here break when the journal path shape or the identity encoding
//! changes; they never change the implementation.
//!
//! Keep this module focused on resolver and identity behavior.

use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

/// A unique absolute directory for one test, never created: this module
/// resolves paths and touches no filesystem, so the name only has to be unique,
/// not real.
fn unique_data_dir(label: &str) -> PathBuf {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "clio-erasure-journal-{label}-{}-{sequence}",
        std::process::id()
    ))
}

/// The injected lookup that answers `name` with `dir` and nothing else.
fn env_dir(name: &'static str, dir: &Path) -> impl Fn(&str) -> Option<String> + use<> {
    let value = dir.to_string_lossy().into_owned();
    move |key: &str| (key == name).then(|| value.clone())
}

#[test]
fn the_explicit_override_wins_over_the_data_directory() {
    let get = |key: &str| match key {
        "CLIO_ERASURE_JOURNAL_FILE" => Some("/keys/erasure-journal.json".to_owned()),
        "CLIO_DATA_DIR" => Some("/data".to_owned()),
        _ => None,
    };

    assert_eq!(
        resolve_journal_path(&get),
        Ok(PathBuf::from("/keys/erasure-journal.json"))
    );
}

#[test]
fn an_explicit_relative_override_is_honoured() {
    let get = |key: &str| (key == ERASURE_JOURNAL_FILE_ENV).then(|| "journal.json".to_owned());

    assert_eq!(
        resolve_journal_path(&get),
        Ok(PathBuf::from("journal.json")),
        "naming a path is the operator's decision rather than a fallback"
    );
}

#[test]
fn a_blank_override_counts_as_unset() {
    for blank in ["", "   ", "\t"] {
        let dir = unique_data_dir("blank");
        let data_dir = dir.to_string_lossy().into_owned();
        let get = move |key: &str| match key {
            "CLIO_ERASURE_JOURNAL_FILE" => Some(blank.to_owned()),
            "CLIO_DATA_DIR" => Some(data_dir.clone()),
            _ => None,
        };

        assert_eq!(
            resolve_journal_path(&get),
            Ok(dir.join(ERASURE_JOURNAL_FILE_NAME)),
            "{blank:?} must fall through to the data directory"
        );
    }
}

#[test]
fn the_default_is_the_data_directory_sibling() {
    let dir = unique_data_dir("data");

    let resolved =
        resolve_journal_path(&env_dir("CLIO_DATA_DIR", &dir)).expect("an absolute data directory");

    assert_eq!(resolved, dir.join(ERASURE_JOURNAL_FILE_NAME));
}

#[test]
fn the_xdg_data_home_tier_resolves_under_the_app_subdirectory() {
    let dir = unique_data_dir("xdg");

    let resolved =
        resolve_journal_path(&env_dir("XDG_DATA_HOME", &dir)).expect("an absolute data directory");

    assert_eq!(resolved, dir.join("clio").join(ERASURE_JOURNAL_FILE_NAME));
}

#[test]
fn the_home_tier_resolves_under_local_share() {
    let dir = unique_data_dir("home");

    let resolved =
        resolve_journal_path(&env_dir("HOME", &dir)).expect("an absolute data directory");

    assert_eq!(
        resolved,
        dir.join(".local")
            .join("share")
            .join("clio")
            .join(ERASURE_JOURNAL_FILE_NAME)
    );
}

#[test]
fn no_data_directory_at_all_is_refused_with_the_override_named() {
    let absent = |_: &str| None;

    let err = resolve_journal_path(&absent).expect_err("nowhere for the journal to live");

    assert_eq!(err.code, ErrorCode::ConfigCorrupt);
    assert!(
        err.message.contains(ERASURE_JOURNAL_FILE_ENV),
        "the refusal names the override that fixes it: {}",
        err.message
    );
}

#[test]
fn a_relative_data_directory_is_refused_rather_than_followed() {
    // A journal beside a relative data directory would land in whatever
    // directory the process started in, outside no replacement path at all.
    let err = resolve_journal_path(&env_dir("CLIO_DATA_DIR", Path::new("relative/dir")))
        .expect_err("a relative data directory is not a home");

    assert_eq!(err.code, ErrorCode::ConfigCorrupt);
}

#[test]
fn the_default_stays_outside_the_paths_a_restore_replaces() {
    let dir = unique_data_dir("placement");

    let resolved =
        resolve_journal_path(&env_dir("CLIO_DATA_DIR", &dir)).expect("an absolute data directory");

    // The database file and its sidecars, and both key-store fallback names,
    // live in this same directory; the journal is a sibling none of them names.
    for replaced in [
        dir.join("clio.db"),
        dir.join("clio.db-wal"),
        dir.join("clio.db-shm"),
        dir.join("clio.db-journal"),
        dir.join("clio.db.kms.json"),
        dir.join("clio.kms.json"),
    ] {
        assert_ne!(resolved, replaced, "the journal must not be {replaced:?}");
    }
    assert!(
        !resolved.starts_with(dir.join("backups")),
        "the journal must not live under the backup directory: {}",
        resolved.display()
    );
}

#[test]
fn a_new_deployment_id_is_64_lowercase_hex_characters() {
    let id = new_deployment_id();

    assert_eq!(id.len(), DEPLOYMENT_ID_LEN * 2);
    assert!(
        id.chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
        "not lowercase hex: {id}"
    );
}

#[test]
fn each_minted_deployment_id_is_a_fresh_value() {
    assert_ne!(
        new_deployment_id(),
        new_deployment_id(),
        "a second mint must not repeat the first"
    );
}
