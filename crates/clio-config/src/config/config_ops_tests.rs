// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for tests of the `config_get` / `config_set` tool
//! handlers on `Runtime`.
//!
//! ## Owns
//! - `config_get` / `config_set` behavior under test for `config::config_ops`
//!
//! ## Does not own
//! - Production `config_get` / `config_set` handlers under test
//! - Overlay merge order, validation rules, or profile/ranking handlers
//!
//! ## Boundary
//!
//! Changes to what `config_get` / `config_set` return should normally update
//! tests here. Changes to merge order or validation rules should not.
//!
//! Keep this module focused on `config_get` / `config_set` handler tests.

use super::*;
use crate::context::BankDefaultPolicy;
use serde_json::json;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn tmp(name: &str) -> PathBuf {
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    std::env::temp_dir().join(format!("am-{name}-{n}"))
}

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
fn config_get_returns_the_masked_tree_for_no_path() {
    let mut rt = Runtime::with_env(&|_| None);
    rt.load_file_json(r#"{"credentials":{"api_key":"sk-ABCDEFG1234"}}"#)
        .expect("load");
    let ToolResult::Ok { data, .. } = rt.config_get(None) else {
        panic!("expected whole tree");
    };
    assert_eq!(data.path, "");
    assert!(data.active_profile.is_none());
    // The whole tree is masked, not only the pointer a caller named.
    assert!(!data.value.to_string().contains("sk-ABCDEFG1234"));
    // An empty path string is the same request as no path at all.
    let ToolResult::Ok { data: empty, .. } = rt.config_get(Some("")) else {
        panic!("expected whole tree");
    };
    assert_eq!(empty.value, data.value);
}

#[test]
fn config_get_unknown_path_is_an_error() {
    let rt = Runtime::with_env(&|_| None);
    let ToolResult::Err { code, .. } = rt.config_get(Some("missing.path")) else {
        panic!("expected err");
    };
    assert_eq!(code, ErrorCode::UnknownPath);
}

#[test]
fn config_set_session_scope_applies_and_masks_a_secret() {
    let mut rt = Runtime::with_env(&|_| None);
    let ToolResult::Ok { data, .. } =
        rt.config_set("log.level", json!("debug"), ConfigScope::Session)
    else {
        panic!("set log level");
    };
    assert_eq!(data.scope, "session");
    assert_eq!(rt.effective_raw()["log"]["level"], json!("debug"));
    // A secret path is masked in the echoed value, never returned raw.
    let ToolResult::Ok { data, .. } = rt.config_set(
        "credentials.api_key",
        json!("sk-ABCDEFG1234"),
        ConfigScope::Session,
    ) else {
        panic!("set secret");
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
fn config_set_profile_scope_requires_an_active_profile() {
    let mut rt = Runtime::with_env(&|_| None);
    let ToolResult::Err { code, .. } =
        rt.config_set("backend", json!("sqlite"), ConfigScope::Profile)
    else {
        panic!("need profile");
    };
    assert_eq!(code, ErrorCode::InvalidArgument);
    // With a profile applied the same set is accepted.
    let ToolResult::Ok { .. } = rt.config_profile_apply("coding_local") else {
        panic!("apply");
    };
    let ToolResult::Ok { data, .. } =
        rt.config_set("backend", json!("postgres"), ConfigScope::Profile)
    else {
        panic!("profile set");
    };
    assert_eq!(data.scope, "profile");
    assert_eq!(rt.effective_raw()["backend"], json!("postgres"));
}

#[test]
fn config_set_deployment_scope_persists_to_the_resolved_file() {
    let path = tmp("deploy-set.json");
    let mut rt = Runtime::with_env(&|_| None);
    rt.set_deployment_path(path.clone());
    let ToolResult::Ok { data, .. } =
        rt.config_set("backend", json!("postgres"), ConfigScope::Deployment)
    else {
        panic!("set");
    };
    assert_eq!(data.scope, "deployment");
    assert_eq!(rt.deployment_field("backend"), Some(&json!("postgres")));
    let written = std::fs::read_to_string(&path).expect("deployment file written");
    assert!(written.contains("postgres"), "{written}");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn config_set_deployment_scope_reports_a_save_failure() {
    let dir = tmp("deploy-dir");
    std::fs::create_dir_all(&dir).expect("dir");
    let mut rt = Runtime::with_env(&|_| None);
    rt.set_deployment_path(dir.clone());
    let ToolResult::Err { code, .. } =
        rt.config_set("backend", json!("postgres"), ConfigScope::Deployment)
    else {
        panic!("expected save err");
    };
    assert_eq!(code, ErrorCode::Internal);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn config_set_updates_the_bank_policy() {
    let mut rt = Runtime::with_env(&|_| None);
    let ToolResult::Ok { .. } = rt.config_set(
        "bank.policy",
        json!({"kind": "fixed", "bank": "fb"}),
        ConfigScope::Session,
    ) else {
        panic!("set policy");
    };
    assert_eq!(
        rt.bank_policy,
        BankDefaultPolicy::Fixed { bank: "fb".into() }
    );
    assert_eq!(rt.effective_raw()["bank"]["policy"]["kind"], json!("fixed"));
}
