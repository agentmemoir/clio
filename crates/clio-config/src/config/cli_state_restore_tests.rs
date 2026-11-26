// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests what a `Runtime` boot restores from a durable CLI state
//! file: the profile, the profile overlay, and the ranking environment, and
//! what a hand-written or outdated file may and may not put back.
//!
//! ## Owns
//! - Behavior tests for the profile and overlay a boot restores
//! - Behavior tests for the checks a loaded overlay entry has to pass
//! - Behavior tests for a ranking object that predates a knob
//!
//! ## Does not own
//! - The implementation under test (`cli_state`)
//! - Path resolution and the write path (see `tests`)
//! - The shared scaffolding these tests use (see `cli_state_test_support`)
//! - The path and value rules themselves (see `validate`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::cli_state_test_support::{
    Scratch, active_of, active_profile_of, boot, log_level_of, ranking_of, set_and_flush, value_at,
    write_overlay_state,
};
use crate::config::ConfigScope;
use crate::error::ToolResult;
use crate::profile::lookup_profile;
use crate::ranking::RankingEnv;
use serde_json::json;
use std::fs;

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
    let default_level = log_level_of(&boot(scratch.path().join("absent.json")));
    assert_eq!(
        log_level_of(&rt),
        default_level,
        "a vanished profile's overlay is not applied"
    );
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
    let baseline = boot(scratch.path().join("absent.json"));
    assert_eq!(
        value_at(&rt, "/gates/admission_enabled"),
        value_at(&baseline, "/gates/admission_enabled"),
        "a forbidden gate path leaves the default in place"
    );
    assert_eq!(
        value_at(&rt, "/gates/admission/enabled"),
        serde_json::Value::Null,
        "a forbidden gate path is not created at all"
    );
    assert_eq!(
        value_at(&rt, "/fr10_satisfied"),
        value_at(&baseline, "/fr10_satisfied"),
        "a packaging claim cannot be written through the file"
    );
    assert_eq!(
        value_at(&rt, "/credentials/api_key"),
        serde_json::Value::Null,
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
    let baseline = boot(scratch.path().join("absent.json"));
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

#[test]
fn a_persisted_profile_outranks_the_environment_log_level_at_boot() {
    let scratch = Scratch::new();
    let path = scratch.state_file();
    let mut first = boot(path.clone());
    let ToolResult::Ok { .. } = first.config_profile_apply("coding_local") else {
        panic!("the profile applies");
    };
    assert!(first.flush_cli_state().expect("flush").written());

    let second = super::Runtime::with_env_and_cli_state_file(
        &|key: &str| (key == "CLIO_LOG_LEVEL").then(|| "trace".to_owned()),
        path,
    );
    let profile = lookup_profile("coding_local").expect("coding_local exists");
    assert_eq!(
        log_level_of(&second),
        json!(profile.log_level),
        "the restored profile's log.level wins over CLIO_LOG_LEVEL"
    );
}
