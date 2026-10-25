// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for behavior tests of the `config_profiles`,
//! `config_profile_apply`, and `ranking_env_*` handlers on `Runtime`: the
//! catalog the list reports, the bundle an apply writes, and the dry-run and
//! applied shapes of a ranking patch.
//!
//! ## Owns
//! - Behavior tests for `config::profile_ops`
//!
//! ## Does not own
//! - The profile catalog itself and its text (see `crate::profile`)
//! - Ranking weight validation math (see `crate::ranking`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;
use crate::config::ConfigScope;
use crate::context::BankDefaultPolicy;
use crate::ranking::RankingEnvPatch;

/// A runtime over no host environment, so no test here reads the machine.
fn runtime() -> Runtime {
    Runtime::with_env(&|_key: &str| None)
}

#[test]
fn config_profiles_lists_every_builtin_profile() {
    let rt = runtime();
    let ToolResult::Ok { data, .. } = rt.config_profiles() else {
        panic!("listing the builtin profiles must succeed");
    };
    let names: Vec<&str> = data.profiles.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, vec!["coding_local", "coding_ci"]);
    assert!(
        data.profiles.iter().all(|p| !p.description.is_empty()),
        "every listed profile carries the description a caller reads"
    );
    assert!(
        data.profiles.iter().all(|p| !p.fr10_satisfied),
        "no listed profile claims packaging satisfied its gates"
    );
    assert_eq!(data.active, None, "nothing is active before an apply");
}

#[test]
fn apply_writes_the_profile_bundle_and_reports_the_change() {
    let mut rt = runtime();
    let ToolResult::Ok { data, .. } = rt.config_profile_apply("coding_ci") else {
        panic!("applying a builtin profile must succeed");
    };
    assert_eq!(data.name, "coding_ci");
    assert!(matches!(data.bank_policy, BankDefaultPolicy::PerRepository));
    assert!(
        !data.fr10_satisfied,
        "an apply never claims the gates are satisfied"
    );
    assert!(data.note.contains("does not bypass"), "{}", data.note);
    assert!(
        !data.tool_inventory.is_empty(),
        "the report names the tools this build binds"
    );
    assert!(
        data.diff
            .as_object()
            .is_some_and(|changed| changed.contains_key("log.level")),
        "the diff names the knob the profile moved"
    );

    let raw = rt.effective_raw();
    assert_eq!(raw.pointer("/profile/active"), Some(&json!("coding_ci")));
    assert_eq!(raw.pointer("/backend"), Some(&json!("sqlite")));
    assert_eq!(
        raw.pointer("/log/level"),
        Some(&json!("warn")),
        "coding_ci is the quieter of the two profiles"
    );
    assert_eq!(raw.pointer("/injection/budget_tokens"), Some(&json!(2048)));
    assert_eq!(raw.pointer("/sync/omit"), Some(&json!(true)));
    assert_eq!(raw.pointer("/mcp/revision"), Some(&json!("2025-11-25")));
    assert_eq!(
        raw.pointer("/mcp/transports"),
        Some(&json!(["stdio", "streamable_http"]))
    );
    assert_eq!(raw.pointer("/fr10_satisfied"), Some(&json!(false)));
}

#[test]
fn unknown_profile_reports_unknown_profile_and_changes_nothing() {
    let mut rt = runtime();
    let before = rt.effective_raw();
    let ToolResult::Err { code, message, .. } = rt.config_profile_apply("no_such_profile") else {
        panic!("an unknown profile must be refused");
    };
    assert_eq!(code, ErrorCode::UnknownProfile);
    assert!(message.contains("no_such_profile"), "{message}");
    assert_eq!(
        rt.effective_raw(),
        before,
        "a refused apply leaves every layer as it was"
    );
}

#[test]
fn applying_a_profile_clears_the_overlay_the_previous_one_saved() {
    let mut rt = runtime();
    let ToolResult::Ok { .. } = rt.config_profile_apply("coding_local") else {
        panic!("first apply");
    };
    let ToolResult::Ok { .. } = rt.config_set("log.level", json!("debug"), ConfigScope::Profile)
    else {
        panic!("profile-scoped set");
    };
    assert_eq!(
        rt.effective_raw().pointer("/log/level"),
        Some(&json!("debug"))
    );

    let ToolResult::Ok { .. } = rt.config_profile_apply("coding_ci") else {
        panic!("second apply");
    };
    assert_eq!(
        rt.effective_raw().pointer("/log/level"),
        Some(&json!("warn")),
        "the replaced profile's overlay must not outrank the new profile's bundle"
    );
}

#[test]
fn ranking_env_dry_run_reports_the_normalized_value_without_applying_it() {
    let mut rt = runtime();
    let before = rt.ranking_env().clone();
    let patch = RankingEnvPatch {
        theta_admit: Some(0.42),
        ..RankingEnvPatch::default()
    };
    let ToolResult::Ok { data, .. } = rt.ranking_env_set(&patch, true) else {
        panic!("a dry run must succeed");
    };
    assert!(data.dry_run);
    assert!(!data.applied);
    assert!((data.ranking.theta_admit - 0.42).abs() < 1e-9);
    assert_eq!(
        rt.ranking_env(),
        &before,
        "a dry run leaves the live ranking untouched"
    );
}

#[test]
fn ranking_env_set_applies_and_the_borrow_sees_the_new_value() {
    let mut rt = runtime();
    let patch = RankingEnvPatch {
        theta_admit: Some(0.42),
        ..RankingEnvPatch::default()
    };
    let ToolResult::Ok { data, .. } = rt.ranking_env_set(&patch, false) else {
        panic!("an applied patch must succeed");
    };
    assert!(data.applied);
    assert!(!data.dry_run);
    assert!(
        (rt.ranking_env().theta_admit - 0.42).abs() < 1e-9,
        "the borrowed ranking is the one a later call uses"
    );
    let ToolResult::Ok { data: got, .. } = rt.ranking_env_get() else {
        panic!("reading the ranking must succeed");
    };
    assert!((got.theta_admit - 0.42).abs() < 1e-9);
}
