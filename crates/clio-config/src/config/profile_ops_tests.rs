// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for tests of the profile list/apply and
//! `ranking_env` handlers on `Runtime`.
//!
//! ## Owns
//! - Profile and `ranking_env` behavior under test for `config::profile_ops`
//!
//! ## Does not own
//! - Production profile or ranking handlers under test
//! - Ranking weight math or config path validation
//!
//! ## Boundary
//!
//! Changes to profile apply side effects or `ranking_env_*` results should
//! normally update tests here. Changes to `config_get` / `config_set` should not.
//!
//! Keep this module focused on profile and `ranking_env` handler tests.

use super::*;
use crate::config::ConfigScope;
use crate::context::BankDefaultPolicy;
use crate::ranking::{AdmissionWeights, RetrievalWeights};
use serde_json::json;

#[test]
fn config_profiles_lists_builtins_and_the_active_one() {
    let mut rt = Runtime::with_env(&|_| None);
    let ToolResult::Ok { data, .. } = rt.config_profiles() else {
        panic!("profiles");
    };
    assert!(data.profiles.iter().any(|p| p.name == "coding_local"));
    assert!(data.active.is_none());
    let ToolResult::Ok { .. } = rt.config_profile_apply("coding_local") else {
        panic!("apply");
    };
    let ToolResult::Ok { data, .. } = rt.config_profiles() else {
        panic!("profiles again");
    };
    assert_eq!(data.active.as_deref(), Some("coding_local"));
}

#[test]
fn profile_apply_sets_per_repo_bank_policy() {
    let mut rt = Runtime::with_env(&|_| None);
    rt.set_repo_root("/tmp/demo-repo");
    let ToolResult::Ok { data, .. } = rt.config_profile_apply("coding_local") else {
        panic!("apply");
    };
    assert_eq!(data.name, "coding_local");
    assert!(!data.fr10_satisfied);
    assert!(matches!(data.bank_policy, BankDefaultPolicy::PerRepository));
    let ctx = rt.resolve_context(None, None).expect("bank");
    assert_eq!(ctx.bank.as_str(), "repo:/tmp/demo-repo");
}

#[test]
fn apply_profile_writes_the_bundle_and_clears_the_profile_overlay() {
    let mut rt = Runtime::with_env(&|_| None);
    let ToolResult::Ok { .. } = rt.config_profile_apply("coding_local") else {
        panic!("apply");
    };
    // A profile-scoped set lands in the profile overlay alone.
    let ToolResult::Ok { .. } = rt.config_set("backend", json!("postgres"), ConfigScope::Profile)
    else {
        panic!("profile set");
    };
    assert_eq!(rt.effective_raw()["backend"], json!("postgres"));
    let profile = lookup_profile("coding_ci").expect("profile");
    let ToolResult::Ok { .. } = rt.config_profile_apply("coding_ci") else {
        panic!("re-apply");
    };
    // Re-applying rebuilds the bundle and drops the previous overlay.
    assert_eq!(rt.active_profile.as_deref(), Some("coding_ci"));
    let effective = rt.effective_raw();
    assert_eq!(effective["backend"], json!(profile.backend_hint));
    assert_eq!(effective["log"]["level"], json!(profile.log_level));
    assert_eq!(
        effective["injection"]["budget_tokens"],
        json!(profile.injection_budget_tokens)
    );
    assert_eq!(effective["sync"]["omit"], json!(profile.omit_sync));
    assert_eq!(effective["mcp"]["revision"], json!(profile.mcp_revision));
    assert_eq!(effective["mcp"]["transports"], json!(profile.mcp_transports));
    assert_eq!(effective["fr10_satisfied"], json!(false));
}

#[test]
fn unknown_profile_leaves_state() {
    let mut rt = Runtime::with_env(&|_| None);
    let ToolResult::Err { code, .. } = rt.config_profile_apply("nope") else {
        panic!("err");
    };
    assert_eq!(code, ErrorCode::UnknownProfile);
    assert!(rt.active_profile.is_none());
}

#[test]
fn ranking_env_get_and_borrow_read_the_live_value() {
    let mut rt = Runtime::with_env(&|_| None);
    let ToolResult::Ok { data, .. } = rt.ranking_env_get() else {
        panic!("get");
    };
    assert_eq!(&data, rt.ranking_env());
    let patch = RankingEnvPatch {
        admission: Some(AdmissionWeights {
            w1: 2.0,
            w2: 1.0,
            w3: 1.0,
            w4: 0.0,
        }),
        ..RankingEnvPatch::default()
    };
    let ToolResult::Ok { .. } = rt.ranking_env_set(&patch, false) else {
        panic!("set");
    };
    // The borrowed view follows the write: a caller that needs the value a wave
    // will use does not unwrap the tool envelope.
    assert_eq!(rt.ranking_env().admission.w1, 0.5);
    let ToolResult::Ok { data, .. } = rt.ranking_env_get() else {
        panic!("get after set");
    };
    assert_eq!(&data, rt.ranking_env());
}

#[test]
fn ranking_dry_run_does_not_apply() {
    let mut rt = Runtime::with_env(&|_| None);
    let before = rt.ranking.clone();
    let patch = RankingEnvPatch {
        retrieval: Some(RetrievalWeights {
            dense: 2.0,
            lexical: 2.0,
            importance: 0.0,
        }),
        ..RankingEnvPatch::default()
    };
    let ToolResult::Ok { data, .. } = rt.ranking_env_set(&patch, true) else {
        panic!("dry");
    };
    assert!(data.dry_run);
    assert!(!data.applied);
    assert!((data.ranking.retrieval.dense - 0.5).abs() < 1e-9);
    assert_eq!(rt.ranking, before);
}

#[test]
fn ranking_set_renormalizes_admission() {
    let mut rt = Runtime::with_env(&|_| None);
    let patch = RankingEnvPatch {
        admission: Some(AdmissionWeights {
            w1: 1.0,
            w2: 1.0,
            w3: 1.0,
            w4: 1.0,
        }),
        ..RankingEnvPatch::default()
    };
    let ToolResult::Ok { data, .. } = rt.ranking_env_set(&patch, false) else {
        panic!("apply");
    };
    assert!(data.applied);
    assert!((data.ranking.admission.w1 - 0.25).abs() < 1e-9);
}

#[test]
fn ranking_set_propagates_an_out_of_range_patch() {
    let mut rt = Runtime::with_env(&|_| None);
    let ToolResult::Err { code, .. } = rt.ranking_env_set(
        &RankingEnvPatch {
            admission: Some(AdmissionWeights {
                w1: -1.0,
                w2: 0.0,
                w3: 0.0,
                w4: 0.0,
            }),
            ..RankingEnvPatch::default()
        },
        false,
    ) else {
        panic!("expected ranking err");
    };
    assert_eq!(code, ErrorCode::OutOfRange);
}
