// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//
//! # Responsibility
//!
//! This module is responsible for unit tests of the config `Runtime` and related tools.
//!
//! ## Owns
//! - Config/ranking/bank acceptance scenarios for this crate module
//!
//! ## Does not own
//! - Production runtime logic under test
//! - Integration tests outside this module tree
//!
//! ## Boundary
//!
//! Changes to expected config/ranking tool behavior under test should normally require changes here.
//! Changes to unrelated crates or subsystems should not require changes here.
//!
//! Keep this module focused on unit tests for the config Runtime tools.

use super::*;
use crate::context::Actor;
use crate::error::ErrorCode;
use crate::ranking::{AdmissionWeights, RankingEnvPatch, RetrievalWeights};
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn config_get_masks_secrets_from_file() {
    let mut rt = Runtime::with_env(&|_| None);
    rt.load_file_json(r#"{"credentials":{"api_key":"sk-ABCDEFG1234"}}"#)
        .expect("load");
    let ToolResult::Ok { data, .. } = rt.config_get(Some("credentials.api_key")) else {
        panic!("expected ok");
    };
    assert_eq!(data.value, json!("****1234"));
}

#[test]
fn config_set_rejects_invalid_log_level() {
    let mut rt = Runtime::with_env(&|_| None);
    let ToolResult::Err { code, .. } =
        rt.config_set("log.level", json!("loud"), ConfigScope::Session)
    else {
        panic!("expected err");
    };
    assert_eq!(code, ErrorCode::OutOfRange);
}

#[test]
fn config_set_rejects_gate_disable() {
    let mut rt = Runtime::with_env(&|_| None);
    let ToolResult::Err { code, .. } = rt.config_set(
        "gates.admission_enabled",
        json!(false),
        ConfigScope::Session,
    ) else {
        panic!("expected err");
    };
    assert_eq!(code, ErrorCode::Forbidden);
}

#[test]
fn config_set_rejects_ranking_path() {
    let mut rt = Runtime::with_env(&|_| None);
    let ToolResult::Err { code, .. } =
        rt.config_set("ranking.theta_admit", json!(0.1), ConfigScope::Session)
    else {
        panic!("expected err");
    };
    assert_eq!(code, ErrorCode::Forbidden);
}

#[test]
fn config_set_rejects_unknown_path() {
    let mut rt = Runtime::with_env(&|_| None);
    let ToolResult::Err { code, .. } = rt.config_set("not.real", json!(1), ConfigScope::Session)
    else {
        panic!("expected err");
    };
    assert_eq!(code, ErrorCode::UnknownPath);
}

#[test]
fn config_set_cannot_reach_the_retention_store() {
    let mut rt = Runtime::with_env(&|_| None);
    let ToolResult::Err { code, .. } = rt.config_set(
        "retention.banks.bank-a.verbosity",
        json!("selective"),
        ConfigScope::Session,
    ) else {
        panic!("expected forbidden");
    };
    assert_eq!(code, ErrorCode::Forbidden);
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
fn bank_fixed_beats_per_repo_policy() {
    let mut rt = Runtime::with_env(&|_| None);
    rt.set_repo_root("/tmp/demo-repo");
    let _ = rt.config_profile_apply("coding_local");
    let ToolResult::Ok { .. } =
        rt.config_set("bank.fixed", json!("override-bank"), ConfigScope::Session)
    else {
        panic!("set");
    };
    let ctx = rt.resolve_context(None, None).expect("bank");
    assert_eq!(ctx.bank.as_str(), "override-bank");
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
fn stub_tool_returns_not_implemented() {
    let mut rt = Runtime::with_env(&|_| None);
    let v = rt.call_tool("store", &json!({}));
    assert_eq!(v["ok"], json!(false));
    assert_eq!(v["code"], json!("not_implemented"));
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
fn required_bank_rejects_when_no_profile() {
    let rt = Runtime::with_env(&|_| None);
    let err = rt.resolve_context(None, None).expect_err("missing");
    assert_eq!(err.code, ErrorCode::MissingBank);
}

#[test]
fn deployment_overlay_persists_to_disk() {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("am-deploy-{nanos}.json"));
    let mut rt = Runtime::with_env(&|_| None);
    rt.set_deployment_path(path.clone());
    let ToolResult::Ok { .. } =
        rt.config_set("backend", json!("postgres"), ConfigScope::Deployment)
    else {
        panic!("set");
    };
    let mut rt2 = Runtime::with_env(&|_| None);
    rt2.set_deployment_path(path.clone());
    rt2.reload_deployment_overlay().expect("reload");
    let ToolResult::Ok { data, .. } = rt2.config_get(Some("backend")) else {
        panic!("get");
    };
    assert_eq!(data.value, json!("postgres"));
    let _ = std::fs::remove_file(path);
}

#[test]
fn deployment_set_without_path_skips_persist() {
    let mut rt = Runtime::with_env(&|_| None);
    rt.clear_deployment_path();
    let ToolResult::Ok { .. } = rt.config_set("backend", json!("sqlite"), ConfigScope::Deployment)
    else {
        panic!("set without path");
    };
    let ToolResult::Ok { data, .. } = rt.config_get(Some("backend")) else {
        panic!("get");
    };
    assert_eq!(data.value, json!("sqlite"));
}

#[test]
fn deployment_error_and_context_actor_paths() {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let bad = std::env::temp_dir().join(format!("am-bad-deploy-{nanos}.json"));
    std::fs::write(&bad, "not-json").expect("write");

    // finish_boot ignores corrupt deployment (if-let Ok miss / Err arm).
    let _ = Runtime::with_deployment_file(&bad);

    // reload_deployment_overlay propagates load Err via `?`.
    let mut rt = Runtime::with_env(&|_| None);
    rt.set_deployment_path(bad.clone());
    assert_eq!(
        rt.reload_deployment_overlay().expect_err("corrupt").code,
        ErrorCode::ConfigCorrupt
    );

    // config_set Deployment propagates save Err when path is not writable.
    let dir = std::env::temp_dir().join(format!("am-deploy-dir-{nanos}"));
    std::fs::create_dir_all(&dir).expect("dir");
    let mut rt2 = Runtime::with_env(&|_| None);
    rt2.set_deployment_path(dir.clone());
    let ToolResult::Err { code, .. } =
        rt2.config_set("backend", json!("postgres"), ConfigScope::Deployment)
    else {
        panic!("expected save err");
    };
    assert_eq!(code, ErrorCode::Internal);

    // resolve_context propagates Actor::parse Err via `?`.
    let rt3 = Runtime::with_env(&|_| None);
    assert_eq!(
        rt3.resolve_context(Some("bank"), Some("nope"))
            .expect_err("actor")
            .code,
        ErrorCode::InvalidArgument
    );

    // ranking_env_set propagates preview_patch Err via `?`.
    let mut rt4 = Runtime::with_env(&|_| None);
    let ToolResult::Err { code, .. } = rt4.ranking_env_set(
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

    let _ = std::fs::remove_file(bad);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn runtime_paths_for_full_surface() {
    let _ = Runtime::default();
    let mut rt = Runtime::with_env(&|_| None);
    assert!(rt.reload_deployment_overlay().is_ok());
    assert!(rt.load_file_json("not-json").is_err());
    assert!(rt.load_file_json("[]").is_err());
    rt.load_file_json(r#"{"log":{"level":"debug"}}"#).unwrap();

    let ToolResult::Ok { .. } = rt.config_profiles() else {
        panic!("profiles");
    };
    let ToolResult::Err { .. } = rt.config_get(Some("missing.path")) else {
        panic!("unknown path");
    };
    let ToolResult::Ok { .. } = rt.config_profile_apply("coding_ci") else {
        panic!("ci");
    };
    let ToolResult::Ok { .. } = rt.config_set("sync.omit", json!(true), ConfigScope::Profile)
    else {
        panic!("profile set");
    };
    let ToolResult::Ok { .. } = rt.config_set(
        "bank.policy",
        json!({"kind": "fixed", "bank": "fb"}),
        ConfigScope::Session,
    ) else {
        panic!("policy");
    };
    let ToolResult::Ok { .. } = rt.config_set(
        "credentials.api_key",
        json!("sk-abcdef"),
        ConfigScope::Session,
    ) else {
        panic!("secret set");
    };
    let ToolResult::Ok { .. } =
        rt.config_set("injection.budget_tokens", json!(100), ConfigScope::Session)
    else {
        panic!("budget");
    };
    let ToolResult::Ok { .. } = rt.config_set("log.level", json!("warn"), ConfigScope::Session)
    else {
        panic!("log");
    };
    let ctx = rt
        .resolve_context(Some("explicit"), Some("harness"))
        .expect("ctx");
    assert_eq!(ctx.bank.as_str(), "explicit");
    assert_eq!(ctx.actor, Actor::Harness);

    let mut bare = Runtime::with_env(&|_| None);
    let ToolResult::Err { code, .. } =
        bare.config_set("backend", json!("sqlite"), ConfigScope::Profile)
    else {
        panic!("need profile");
    };
    assert_eq!(code, ErrorCode::InvalidArgument);

    // Invalid log level via file (bypasses config_set validation) exercises parse-None.
    let mut rt3 = Runtime::with_env(&|_| None);
    rt3.load_file_json(r#"{"log":{"level":"not-a-level"}}"#)
        .unwrap();

    let deploy = std::env::temp_dir().join(format!(
        "am-env-deploy-{}.json",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("t")
            .as_nanos()
    ));
    std::fs::write(&deploy, r#"{"backend":"sqlite"}"#).unwrap();
    let rt_boot = Runtime::with_deployment_file(&deploy);
    let ToolResult::Ok { data, .. } = rt_boot.config_get(Some("backend")) else {
        panic!("deploy boot");
    };
    assert_eq!(data.value, json!("sqlite"));

    let mut rt_env = Runtime::with_env(&|_| None);
    // Explicit None for every key so if-let miss arms are covered regardless of host env.
    rt_env.apply_process_env(&mut |_| None);
    rt_env.apply_process_env(&mut |key| match key {
        "CLIO_LOG_LEVEL" => Some("debug".into()),
        "CLIO_BACKEND" => Some("postgres".into()),
        "CLIO_DATABASE_URL" => Some("postgres://agentmemoir@127.0.0.1/agentmemoir".into()),
        "CLIO_BANK" => Some("env-bank".into()),
        "CLIO_REPO_ROOT" => Some("/tmp/from-env".into()),
        _ => None,
    });
    let ctx = rt_env.resolve_context(None, None).expect("env bank");
    assert_eq!(ctx.bank.as_str(), "env-bank");
    assert_eq!(rt_env.repo_root.as_deref(), Some("/tmp/from-env"));
    assert_eq!(
        rt_env.effective_raw()["store"]["database_url"],
        "postgres://agentmemoir@127.0.0.1/agentmemoir"
    );
    // Masked view must redact database_url.
    let masked = rt_env.effective_masked();
    let masked_url = masked["store"]["****_url"].as_str().unwrap_or("");
    assert!(masked_url.contains("****"), "{masked}");
    assert_ne!(masked_url, "postgres://agentmemoir@127.0.0.1/agentmemoir");
    let _ = std::fs::remove_file(deploy);
}

#[test]
fn history_policy_reads_defaults_and_overrides() {
    let defaults = crate::HistoryKnobs::default();
    let mut rt = Runtime::with_env(&|_| None);
    let policy = rt.history_policy().expect("default history policy");
    assert_eq!(policy.lesson_max_tokens, defaults.lesson_max_tokens);
    assert_eq!(policy.max_limit, defaults.max_limit);

    let ToolResult::Ok { .. } =
        rt.config_set("history.lesson_max_tokens", json!(40), ConfigScope::Session)
    else {
        panic!("set lesson cap");
    };
    let ToolResult::Ok { .. } =
        rt.config_set("history.default_limit", json!(3), ConfigScope::Session)
    else {
        panic!("set default limit");
    };
    let policy = rt.history_policy().expect("tuned history policy");
    assert_eq!(policy.lesson_max_tokens, 40);
    assert_eq!(policy.default_limit, 3);

    // An invalid combination surfaces as an error rather than panicking.
    let ToolResult::Ok { .. } =
        rt.config_set("history.default_limit", json!(300), ConfigScope::Session)
    else {
        panic!("set over-max limit");
    };
    assert!(rt.history_policy().is_err());
}
