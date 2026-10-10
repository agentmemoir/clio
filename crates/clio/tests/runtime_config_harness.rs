// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//! Runtime/config/ranking integration harness: acceptance scenarios + CLI smoke.

use clio::config::ConfigScope;
use clio::error::{ErrorCode, ToolResult};
use clio::ranking::{AdmissionWeights, RankingEnvPatch, RetrievalWeights};
use clio::{Actor, Runtime};
use serde_json::json;
use std::process::Command;

fn clio_bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_clio"))
}

#[test]
fn t01_cli_version_and_health_smoke() {
    let version = clio_bin()
        .arg("version")
        .output()
        .expect("run clio version");
    assert!(version.status.success());
    let stdout = String::from_utf8_lossy(&version.stdout);
    assert!(stdout.contains("clio"));

    let health = clio_bin().arg("health").output().expect("run clio health");
    assert!(health.status.success());
    let body: serde_json::Value = serde_json::from_slice(&health.stdout).expect("health json");
    assert_eq!(body["ok"], json!(true));
    assert_eq!(body["name"], json!("clio"));
}

#[test]
fn t02_config_get_after_overlays_masks_secrets() {
    let mut rt = Runtime::with_env(&|_| None);
    rt.load_file_json(r#"{"credentials":{"api_key":"sk-SECRETKEY99"},"backend":"sqlite"}"#)
        .expect("load");
    let ToolResult::Ok { data, .. } = rt.config_get(Some("credentials.api_key")) else {
        panic!("get");
    };
    assert_eq!(data.value, json!("****EY99"));
    let ToolResult::Ok { data, .. } = rt.config_get(Some("backend")) else {
        panic!("backend");
    };
    assert_eq!(data.value, json!("sqlite"));
}

#[test]
fn t03_config_set_invalid_rejected_no_partial() {
    let mut rt = Runtime::with_env(&|_| None);
    let before = rt.effective_masked();
    let ToolResult::Err { code, .. } =
        rt.config_set("log.level", json!("nope"), ConfigScope::Session)
    else {
        panic!("err");
    };
    assert_eq!(code, ErrorCode::OutOfRange);
    assert_eq!(rt.effective_masked(), before);
}

#[test]
fn t04_config_profile_apply_coding_local() {
    let mut rt = Runtime::with_env(&|_| None);
    rt.set_repo_root("/tmp/clio-runtime-config-harness");
    let ToolResult::Ok { data, .. } = rt.config_profile_apply("coding_local") else {
        panic!("apply");
    };
    assert!(!data.diff.is_null());
    assert!(!data.fr10_satisfied);
    assert!(
        data.tool_inventory
            .iter()
            .any(|t| t.name == "store" && t.implemented)
    );
    assert!(
        data.tool_inventory
            .iter()
            .any(|t| t.name == "config_get" && t.implemented)
    );
    let ctx = rt.resolve_context(None, Some("agent")).expect("ctx");
    assert_eq!(ctx.bank.as_str(), "repo:/tmp/clio-runtime-config-harness");
    assert_eq!(ctx.actor, Actor::Agent);
}

#[test]
fn t05_ranking_env_set_renormalizes() {
    let mut rt = Runtime::with_env(&|_| None);
    let patch = RankingEnvPatch {
        admission: Some(AdmissionWeights {
            w1: 3.0,
            w2: 1.0,
            w3: 0.0,
            w4: 0.0,
            w5: 0.0,
        }),
        ..RankingEnvPatch::default()
    };
    let ToolResult::Ok { data, .. } = rt.ranking_env_set(&patch, false) else {
        panic!("set");
    };
    assert!((data.ranking.admission.w1 - 0.75).abs() < 1e-9);
    assert!((data.ranking.admission.w2 - 0.25).abs() < 1e-9);
}

#[test]
fn t06_ranking_env_set_dry_run_no_write() {
    let mut rt = Runtime::with_env(&|_| None);
    let before = rt.ranking_env_get();
    let ToolResult::Ok {
        data: before_data, ..
    } = before
    else {
        panic!("get");
    };
    let patch = RankingEnvPatch {
        retrieval: Some(RetrievalWeights {
            dense: 1.0,
            lexical: 1.0,
            importance: 1.0,
        }),
        ..RankingEnvPatch::default()
    };
    let ToolResult::Ok { data, .. } = rt.ranking_env_set(&patch, true) else {
        panic!("dry");
    };
    assert!(data.dry_run && !data.applied);
    let ToolResult::Ok { data: after, .. } = rt.ranking_env_get() else {
        panic!("after");
    };
    assert_eq!(after, before_data);
}

#[test]
fn t07_missing_bank_policy_paths() {
    let rt = Runtime::with_env(&|_| None);
    let err = rt.resolve_context(None, None).expect_err("required");
    assert_eq!(err.code, ErrorCode::MissingBank);

    let mut coding = Runtime::with_env(&|_| None);
    coding.set_repo_root("/tmp/forced");
    let _ = coding.config_profile_apply("coding_local");
    let ctx = coding.resolve_context(None, None).expect("forced default");
    assert_eq!(ctx.bank.as_str(), "repo:/tmp/forced");
}
