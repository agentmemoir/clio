// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the durable CLI state file's path resolution, its ranking
//! round trip across two `Runtime` boots, the missing and corrupt cases, and
//! the permissions of the directory a flush creates.
//!
//! ## Owns
//! - Behavior tests for path resolution and the ranking round trip
//! - Behavior tests for what a flush does and does not write
//! - Behavior tests for the mode of the directory the state file lands in
//!
//! ## Does not own
//! - The implementation under test (`cli_state`)
//! - What a hand-written file may restore (see `restore_tests`)
//! - The shared scaffolding these tests use (see `cli_state_test_support`)
//! - Ranking weight math (see `crate::ranking`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::cli_state_test_support::{Scratch, boot, ranking_of, set_and_flush, theta_patch};
use super::{Runtime, resolve_cli_state_path, resolve_cli_state_path_with};
use crate::config::ConfigScope;
use crate::error::ToolResult;
use crate::ranking::RankingEnv;
use serde_json::json;
use std::fs;
use std::path::PathBuf;

#[test]
fn explicit_path_wins_and_the_default_hangs_off_the_config_dir() {
    let explicit = PathBuf::from("/state/cli-state.json");
    assert_eq!(
        resolve_cli_state_path(Some(explicit.clone())),
        Some(explicit.clone())
    );
    assert_eq!(
        resolve_cli_state_path_with(Some(explicit.clone()), &|_| None),
        Some(explicit)
    );
    let xdg = |key: &str| (key == "XDG_CONFIG_HOME").then(|| "/xdg".to_owned());
    assert_eq!(
        resolve_cli_state_path_with(None, &xdg),
        Some(PathBuf::from("/xdg/clio/cli-state.json"))
    );
    assert_eq!(resolve_cli_state_path_with(None, &|_| None), None);
}

#[test]
fn missing_file_boots_the_system_defaults() {
    let scratch = Scratch::new();
    assert_eq!(
        ranking_of(&boot(scratch.state_file())),
        RankingEnv::system_defaults()
    );
}

#[test]
fn corrupt_file_is_ignored() {
    let scratch = Scratch::new();
    let path = scratch.state_file();
    for text in ["not json", "[]", r#"{"ranking": {"theta_admit": "high"}}"#] {
        fs::write(&path, text).expect("write corrupt cli state");
        assert_eq!(
            ranking_of(&boot(path.clone())),
            RankingEnv::system_defaults(),
            "corrupt file {text} must be ignored"
        );
    }
}

#[test]
fn a_later_boot_reads_the_ranking_a_flush_saved() {
    let scratch = Scratch::new();
    let path = scratch.state_file();
    set_and_flush(&mut boot(path.clone()), 0.61);

    let reloaded = ranking_of(&boot(path));
    assert!((reloaded.theta_admit - 0.61).abs() < 1e-9, "{reloaded:?}");
    assert_eq!(
        reloaded.retrieval,
        RankingEnv::system_defaults().retrieval,
        "the whole environment round-trips, not just the patched field"
    );
}

#[test]
fn a_dry_run_writes_nothing_and_leaves_the_saved_value_alone() {
    let scratch = Scratch::new();
    let path = scratch.state_file();
    let mut rt = boot(path.clone());

    // No file yet: a dry run never creates one.
    let ToolResult::Ok { data, .. } = rt.ranking_env_set(&theta_patch(0.7), true) else {
        panic!("dry run must succeed");
    };
    assert!(!data.applied, "a dry run reports applied false");
    assert!(!path.exists(), "a dry run must not create the file");

    // With a saved value, a dry run leaves the stored bytes untouched.
    set_and_flush(&mut rt, 0.61);
    let saved = fs::read_to_string(&path).expect("read cli state");
    let _ = rt.ranking_env_set(&theta_patch(0.7), true);
    assert_eq!(fs::read_to_string(&path).expect("read cli state"), saved);
    assert!((ranking_of(&boot(path)).theta_admit - 0.61).abs() < 1e-9);
}

#[test]
fn flushing_without_a_resolved_path_writes_nothing() {
    let rt = Runtime::with_env(&|_key: &str| None);
    let flushed = rt.flush_cli_state().expect("no path is not an error");
    assert!(!flushed.written());
    assert_eq!(flushed.dropped().len(), 0, "no path means nothing to drop");
}

#[test]
fn a_flush_reports_the_secret_profile_value_it_cannot_save() {
    let scratch = Scratch::new();
    let path = scratch.state_file();
    let mut rt = boot(path.clone());
    let ToolResult::Ok { .. } = rt.config_profile_apply("coding_local") else {
        panic!("the profile applies");
    };
    let ToolResult::Ok { .. } = rt.config_set(
        "credentials.api_key",
        json!("sk-abcdef0123456789"),
        ConfigScope::Profile,
    ) else {
        panic!("a profile-scoped secret still applies in memory");
    };

    let flushed = rt.flush_cli_state().expect("flush");
    assert!(flushed.written(), "the rest of the state is saved");
    assert_eq!(
        flushed
            .dropped()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["credentials.api_key"],
        "the caller is told which key was left out"
    );
    let saved = fs::read_to_string(&path).expect("read cli state");
    assert!(!saved.contains("sk-abcdef0123456789"), "{saved}");
    assert!(!saved.contains("credentials.api_key"), "{saved}");
}

#[test]
fn a_profile_url_with_userinfo_is_not_saved_in_clear() {
    let scratch = Scratch::new();
    let mut rt = boot(scratch.state_file());
    let _ = rt.config_profile_apply("coding_local");
    for url in ["https://user:pw@host/v1", "https://host/v1"] {
        let set = rt.config_set("embed.url", json!(url), ConfigScope::Profile);
        assert!(matches!(set, ToolResult::Ok { .. }), "embed.url applies");
        let _ = rt.flush_cli_state().expect("flush");
        let saved = fs::read_to_string(scratch.state_file()).expect("read");
        assert_eq!(saved.contains(url), !url.contains("pw@"), "{saved}");
    }
}

/// A flush that creates the state file's directory makes it owner-only, so the
/// state file never lands in a directory another account can list or write.
#[cfg(unix)]
#[test]
fn a_flush_creates_the_state_directory_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = Scratch::new();
    let parent = scratch.path().join("config");
    let path = parent.join(super::CLI_STATE_FILE_NAME);

    let flushed = boot(path).flush_cli_state().expect("flush");

    assert!(flushed.written(), "the flush creates the file and its dir");
    let mode = fs::metadata(&parent).expect("meta").permissions().mode();
    assert_eq!(
        mode & 0o777,
        0o700,
        "a created state directory must be owner-only, got {mode:o}"
    );
}

/// A directory that a flush finds already present keeps the mode it had: the
/// owner-only rule applies to creation, and a pre-existing wider mode is the
/// operator's choice rather than something a write silently overrides.
#[cfg(unix)]
#[test]
fn a_flush_leaves_an_existing_state_directory_mode_unchanged() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = Scratch::new();
    let parent = scratch.path().join("config");
    fs::create_dir_all(&parent).expect("dir");
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o750)).expect("mode");

    let flushed = boot(parent.join(super::CLI_STATE_FILE_NAME))
        .flush_cli_state()
        .expect("flush");

    assert!(flushed.written(), "the flush writes into the existing dir");
    let mode = fs::metadata(&parent).expect("meta").permissions().mode();
    assert_eq!(
        mode & 0o777,
        0o750,
        "an existing state directory keeps its mode, got {mode:o}"
    );
}
