// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the durable CLI state file: path resolution, the ranking
//! round trip across two `Runtime` boots, the missing and corrupt cases, and
//! what a hand-written file may and may not restore.
//!
//! ## Owns
//! - Behavior tests for the CLI state file and its `Runtime` hooks
//! - Behavior tests for the checks a loaded overlay entry has to pass
//!
//! ## Does not own
//! - The implementation under test (`cli_state`)
//! - The path and value rules themselves (see `validate`)
//! - Ranking weight math (see `crate::ranking`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;
use crate::config::ConfigScope;
use crate::error::ToolResult;
use crate::ranking::RankingEnvPatch;
use serde_json::json;
use std::sync::atomic::{AtomicU64, Ordering};

/// Per-test scratch directory, removed on drop and unique by process and order.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("clio-cli-state-{}-{seq}", std::process::id()));
        fs::create_dir_all(&dir).expect("create scratch dir");
        Self(dir)
    }

    fn state_file(&self) -> PathBuf {
        self.0.join(CLI_STATE_FILE_NAME)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A patch that changes only the admission threshold.
fn theta_patch(theta: f64) -> RankingEnvPatch {
    RankingEnvPatch {
        theta_admit: Some(theta),
        ..RankingEnvPatch::default()
    }
}

/// Boot a runtime over `path` with no host environment and no deployment overlay.
fn boot(path: PathBuf) -> Runtime {
    Runtime::with_env_and_cli_state_file(&|_key: &str| None, path)
}

/// The ranking environment a runtime reports.
fn ranking_of(rt: &Runtime) -> RankingEnv {
    let ToolResult::Ok { data, .. } = rt.ranking_env_get() else {
        panic!("ranking_env_get must succeed");
    };
    data
}

/// Set `theta_admit` on a runtime, both in memory and on disk.
fn set_and_flush(rt: &mut Runtime, theta: f64) {
    let ToolResult::Ok { data, .. } = rt.ranking_env_set(&theta_patch(theta), false) else {
        panic!("ranking_env_set must succeed");
    };
    assert!(data.applied, "a non-dry-run set applies in memory");
    let flushed = rt.flush_cli_state().expect("flush");
    assert!(flushed.written(), "flush must write");
    assert!(
        flushed.dropped().is_empty(),
        "a ranking flush drops nothing"
    );
}

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

/// The profile an `effective_raw` read reports as active.
fn active_of(rt: &Runtime) -> Option<String> {
    rt.effective_raw()
        .pointer("/profile/active")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// The `log.level` a runtime's effective config carries.
fn log_level_of(rt: &Runtime) -> Value {
    rt.effective_raw()
        .pointer("/log/level")
        .cloned()
        .unwrap_or(Value::Null)
}

#[test]
fn a_later_boot_reports_the_profile_and_profile_value_a_flush_saved() {
    let scratch = Scratch::new();
    let path = scratch.state_file();

    let mut first = boot(path.clone());
    let ToolResult::Ok { .. } = first.config_profile_apply("coding_local") else {
        panic!("the profile applies");
    };
    let ToolResult::Ok { .. } = first.config_set("log.level", json!("debug"), ConfigScope::Profile)
    else {
        panic!("a profile-scoped set applies once a profile is active");
    };
    // An explicit ranking set wins over the profile's own ranking.
    set_and_flush(&mut first, 0.61);

    let second = boot(path);
    assert_eq!(active_profile_of(&second).as_deref(), Some("coding_local"));
    assert_eq!(active_of(&second).as_deref(), Some("coding_local"));
    assert_eq!(log_level_of(&second), json!("debug"));
    assert!((ranking_of(&second).theta_admit - 0.61).abs() < 1e-9);
}

#[test]
fn a_persisted_profile_that_no_longer_exists_is_skipped() {
    let scratch = Scratch::new();
    let path = scratch.state_file();
    let mut ranking = serde_json::to_value(RankingEnv::system_defaults()).expect("serialize");
    ranking["theta_admit"] = json!(0.61);
    let saved = json!({
        "active_profile": "a_profile_that_was_removed",
        "profile_overlay": {"log.level": "debug"},
        "ranking": ranking,
    });
    fs::write(
        &path,
        serde_json::to_string(&saved).expect("serialize state"),
    )
    .expect("write state");

    let rt = boot(path);
    assert_eq!(
        active_of(&rt),
        None,
        "an unknown profile leaves no active one"
    );
    assert!(
        (ranking_of(&rt).theta_admit - 0.61).abs() < 1e-9,
        "the rest of the state still loads"
    );
    // A runtime with no state file at all: the comparison the stale profile needs.
    let default_level = log_level_of(&boot(scratch.0.join("absent.json")));
    assert_eq!(
        log_level_of(&rt),
        default_level,
        "a vanished profile's overlay is not applied"
    );
}

/// The active profile `config_profiles` reports.
fn active_profile_of(rt: &Runtime) -> Option<String> {
    let ToolResult::Ok { data, .. } = rt.config_profiles() else {
        panic!("config_profiles must succeed");
    };
    data.active
}

/// Write a state file carrying `overlay`, under the profile that overlay
/// belongs to, so the restore path actually applies it.
fn write_overlay_state(path: &Path, overlay: &Value) {
    let saved = json!({
        "active_profile": "coding_local",
        "profile_overlay": overlay,
    });
    fs::write(
        path,
        serde_json::to_string(&saved).expect("serialize state"),
    )
    .expect("write state");
}

/// The effective value at `pointer`, for comparing a loaded file with a boot
/// that had no file at all.
fn value_at(rt: &Runtime, pointer: &str) -> Value {
    rt.effective_raw()
        .pointer(pointer)
        .cloned()
        .unwrap_or(Value::Null)
}

/// A file that carries a gate key, a packaging claim, or a secret path must not
/// reach the effective config through the overlay: `config set` refuses those
/// paths, and this file is not sealed.
#[test]
fn a_forbidden_or_secret_overlay_entry_is_not_restored_from_the_file() {
    let scratch = Scratch::new();
    let path = scratch.state_file();
    write_overlay_state(
        &path,
        &json!({
            "gates.admission_enabled": false,
            "gates.admission.enabled": false,
            "fr10_satisfied": true,
            "credentials.api_key": "sk-handwritten-0123456789",
            "log.level": "debug",
        }),
    );

    let rt = boot(path);
    let baseline = boot(scratch.0.join("absent.json"));
    assert_eq!(
        value_at(&rt, "/gates/admission_enabled"),
        value_at(&baseline, "/gates/admission_enabled"),
        "a forbidden gate path leaves the default in place"
    );
    assert_eq!(
        value_at(&rt, "/gates/admission/enabled"),
        Value::Null,
        "a forbidden gate path is not created at all"
    );
    assert_eq!(
        value_at(&rt, "/fr10_satisfied"),
        value_at(&baseline, "/fr10_satisfied"),
        "a packaging claim cannot be written through the file"
    );
    assert_eq!(
        value_at(&rt, "/credentials/api_key"),
        Value::Null,
        "the unsealed file must never carry a secret"
    );
    assert_eq!(
        value_at(&rt, "/log/level"),
        json!("debug"),
        "the valid entry beside them still loads"
    );
}

/// A well-allowlisted key with a value of the wrong type is dropped, and the
/// rest of the overlay still applies.
#[test]
fn a_wrong_typed_overlay_value_is_dropped_and_the_valid_entry_loads() {
    let scratch = Scratch::new();
    let path = scratch.state_file();
    write_overlay_state(
        &path,
        &json!({
            "history.default_limit": "many",
            "log.level": "debug",
        }),
    );

    let rt = boot(path);
    let baseline = boot(scratch.0.join("absent.json"));
    assert_eq!(
        value_at(&rt, "/history/default_limit"),
        value_at(&baseline, "/history/default_limit"),
        "a wrong-typed value leaves the default in place"
    );
    assert_eq!(
        value_at(&rt, "/log/level"),
        json!("debug"),
        "the valid entry beside it still loads"
    );
}

/// A ranking object that predates a knob still restores the values it carries
/// instead of being thrown away whole.
#[test]
fn a_saved_ranking_env_missing_a_group_still_loads_from_the_file() {
    let scratch = Scratch::new();
    let path = scratch.state_file();
    let mut ranking = serde_json::to_value(RankingEnv::system_defaults()).expect("serialize");
    ranking["theta_admit"] = json!(0.61);
    ranking
        .as_object_mut()
        .expect("the ranking env is an object")
        .remove("maintenance_triggers");
    fs::write(
        &path,
        serde_json::to_string(&json!({ "ranking": ranking })).expect("serialize state"),
    )
    .expect("write state");

    let restored = ranking_of(&boot(path));
    assert!(
        (restored.theta_admit - 0.61).abs() < 1e-9,
        "the present value survives: {restored:?}"
    );
    assert_eq!(
        restored.maintenance_triggers,
        RankingEnv::system_defaults().maintenance_triggers,
        "the omitted group falls back to its default"
    );
}
