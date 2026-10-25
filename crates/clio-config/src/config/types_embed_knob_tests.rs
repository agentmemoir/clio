// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the `embed.*` / `rerank.*` configuration knobs.
//!
//! ## Owns
//! - Defaults, environment wiring, allowlist, typed checks, secret masking, and
//!   bearer precedence for the embed/rerank sidecar keys
//!
//! ## Does not own
//! - Production config assembly and validation (`validate`/`types`)
//! - Embedding transport behavior (owned by the `clio-index` crate)
//!
//! ## Boundary
//!
//! Changes to embed config-key semantics should normally update tests here.
//! Changes to sidecar transport should not require changes here.
//!
//! Keep this module focused on embed config-key behavior.

use crate::config::*;
use crate::error::ErrorCode;
use serde_json::json;

#[test]
fn embed_config_keys_defaults_allowlist_and_masking() {
    let rt = Runtime::with_env(&|_| None);
    // Defaults expose the Profile B/C embedding contract.
    let eff = rt.effective_raw();
    assert_eq!(eff["embed"]["model"], "BAAI/bge-small-en-v1.5");
    assert_eq!(eff["embed"]["dims"], 384);
    assert_eq!(eff["embed"]["url"], "");
    // Env wiring mirrors the compose/.env contract.
    let mut rt_env = Runtime::with_env(&|_| None);
    rt_env.apply_process_env(&mut |key| match key {
        "CLIO_EMBED_URL" => Some("http://127.0.0.1:34311".into()),
        "CLIO_EMBED_MODEL" => Some("BAAI/bge-small-en-v1.5".into()),
        _ => None,
    });
    let eff_env = rt_env.effective_raw();
    assert_eq!(eff_env["embed"]["url"], "http://127.0.0.1:34311");
    assert_eq!(eff_env["embed"]["model"], "BAAI/bge-small-en-v1.5");

    // Allowlisted embed keys are settable; typed checks hold.
    let mut rt_set = Runtime::with_env(&|_| None);
    rt_set.clear_deployment_path();
    let ToolResult::Ok { .. } = rt_set.config_set(
        "embed.url",
        json!("http://127.0.0.1:34311"),
        ConfigScope::Session,
    ) else {
        panic!("embed.url settable");
    };
    let ToolResult::Ok { .. } = rt_set.config_set("embed.dims", json!(384), ConfigScope::Session)
    else {
        panic!("embed.dims set");
    };
    let ToolResult::Err { code, .. } =
        rt_set.config_set("embed.dims", json!(0), ConfigScope::Session)
    else {
        panic!("embed.dims 0 rejected");
    };
    assert_eq!(code, ErrorCode::OutOfRange);
    let ToolResult::Err { code, .. } =
        rt_set.config_set("embed.dims", json!("384"), ConfigScope::Session)
    else {
        panic!("embed.dims string rejected");
    };
    assert_eq!(code, ErrorCode::InvalidType);

    // Embed API key is a secret path: settable, masked on read.
    let ToolResult::Ok { data, .. } = rt_set.config_set(
        "credentials.embed_api_key",
        json!("sk-sidecar-token-1234567890"),
        ConfigScope::Session,
    ) else {
        panic!("embed api key set");
    };
    assert!(
        data.value
            .as_str()
            .expect("masked set ack")
            .contains("****")
    );
    let masked = rt_set.effective_masked();
    let key_view = masked["****ials"]["****_key"].as_str().unwrap_or("");
    assert!(
        key_view.contains("****") && !key_view.contains("sk-sidecar-token"),
        "{masked}"
    );
}

#[test]
fn embed_rerank_env_knobs_reach_effective_config() {
    let mut rt = Runtime::with_env(&|_| None);
    rt.apply_process_env(&mut |key| match key {
        "CLIO_EMBED_API_KEY" => Some("sk-embed-token-1234567890".into()),
        "CLIO_EMBED_PROVIDER" => Some("tei".into()),
        "CLIO_EMBED_DIMS" => Some("768".into()),
        "CLIO_RERANK_URL" => Some("http://127.0.0.1:34312".into()),
        "CLIO_RERANK_MODEL" => Some("BAAI/bge-reranker-base".into()),
        "CLIO_RERANK_PROVIDER" => Some("cohere".into()),
        "CLIO_RERANK_API_KEY" => Some("sk-rerank-token-1234567890".into()),
        _ => None,
    });
    let eff = rt.effective_raw();
    assert_eq!(eff["embed"]["provider"], "tei");
    assert_eq!(eff["embed"]["dims"], 768);
    assert_eq!(
        eff["credentials"]["embed_api_key"],
        "sk-embed-token-1234567890"
    );
    assert_eq!(eff["rerank"]["url"], "http://127.0.0.1:34312");
    assert_eq!(eff["rerank"]["model"], "BAAI/bge-reranker-base");
    assert_eq!(eff["rerank"]["provider"], "cohere");
    assert_eq!(
        eff["credentials"]["rerank_api_key"],
        "sk-rerank-token-1234567890"
    );
    // Env-sourced keys mask in the masked view too.
    let masked = serde_json::to_string(&rt.effective_masked()).unwrap();
    assert!(!masked.contains("sk-embed-token") && !masked.contains("sk-rerank-token"));
}

#[test]
fn embed_dims_invalid_env_leaves_default() {
    let mut rt = Runtime::with_env(&|_| None);
    rt.apply_process_env(&mut |key| match key {
        "CLIO_EMBED_DIMS" => Some("not-a-number".into()),
        _ => None,
    });
    assert_eq!(rt.effective_raw()["embed"]["dims"], 384);
}

#[test]
fn rerank_defaults_and_settable_all_scopes() {
    let rt = Runtime::with_env(&|_| None);
    let eff = rt.effective_raw();
    assert_eq!(eff["embed"]["provider"], "");
    assert_eq!(
        eff["rerank"],
        json!({"url": "", "model": "", "provider": "", "max_batch": 32})
    );

    let mut rt_set = Runtime::with_env(&|_| None);
    rt_set.clear_deployment_path();
    let ToolResult::Ok { .. } = rt_set.config_profile_apply("coding_ci") else {
        panic!("apply profile");
    };
    for scope in [
        ConfigScope::Session,
        ConfigScope::Profile,
        ConfigScope::Deployment,
    ] {
        let ToolResult::Ok { .. } =
            rt_set.config_set("rerank.url", json!("http://127.0.0.1:34312"), scope)
        else {
            panic!("rerank.url settable scope={}", scope.as_str());
        };
        let ToolResult::Ok { .. } = rt_set.config_set("rerank.provider", json!("tei"), scope)
        else {
            panic!("rerank.provider settable scope={}", scope.as_str());
        };
        let ToolResult::Ok { .. } = rt_set.config_set("embed.provider", json!("openai"), scope)
        else {
            panic!("embed.provider settable scope={}", scope.as_str());
        };
    }
    let ToolResult::Err { code, .. } =
        rt_set.config_set("rerank.nope", json!("x"), ConfigScope::Session)
    else {
        panic!("unknown rerank path rejected");
    };
    assert_eq!(code, ErrorCode::UnknownPath);
}

#[test]
fn bearer_for_precedence() {
    let mut rt = Runtime::with_env(&|_| None);
    assert_eq!(rt.bearer_for("embed"), None);
    assert_eq!(rt.bearer_for("rerank"), None);

    rt.session_overlay
        .insert("credentials.api_key".into(), json!("generic-key-1"));
    assert_eq!(rt.bearer_for("embed"), Some("generic-key-1".to_owned()));
    assert_eq!(rt.bearer_for("rerank"), Some("generic-key-1".to_owned()));

    rt.session_overlay
        .insert("credentials.embed_api_key".into(), json!("embed-key-1"));
    rt.session_overlay
        .insert("credentials.rerank_api_key".into(), json!("rerank-key-1"));
    assert_eq!(rt.bearer_for("embed"), Some("embed-key-1".to_owned()));
    assert_eq!(rt.bearer_for("rerank"), Some("rerank-key-1".to_owned()));

    // Empty strings count as absent: fall back to the generic key.
    rt.session_overlay
        .insert("credentials.embed_api_key".into(), json!(""));
    assert_eq!(rt.bearer_for("embed"), Some("generic-key-1".to_owned()));
}

#[test]
fn rerank_api_key_masked_everywhere() {
    let mut rt = Runtime::with_env(&|_| None);
    rt.clear_deployment_path();
    let ToolResult::Ok { data, .. } = rt.config_set(
        "credentials.rerank_api_key",
        json!("sk-rerank-token-9876543210"),
        ConfigScope::Session,
    ) else {
        panic!("rerank api key set");
    };
    assert!(data.value.as_str().unwrap_or("").contains("****"));
    let ToolResult::Ok { data: get_data, .. } = rt.config_get(Some("credentials.rerank_api_key"))
    else {
        panic!("rerank api key get");
    };
    let get_str = serde_json::to_string(&get_data.value).unwrap();
    assert!(get_str.contains("****") && !get_str.contains("sk-rerank-token"));
    let masked = serde_json::to_string(&rt.effective_masked()).unwrap();
    assert!(!masked.contains("sk-rerank-token-9876543210"));
}

#[test]
fn embed_switch_space_default_and_ceiling() {
    let rt = Runtime::with_env(&|_| None);
    assert_eq!(rt.effective_raw()["embed"]["switch_space"], false);

    let mut rt_set = Runtime::with_env(&|_| None);
    rt_set.clear_deployment_path();
    let ToolResult::Ok { .. } =
        rt_set.config_set("embed.switch_space", json!(true), ConfigScope::Session)
    else {
        panic!("embed.switch_space settable");
    };
    assert_eq!(rt_set.effective_raw()["embed"]["switch_space"], true);
    let ToolResult::Err { code, .. } =
        rt_set.config_set("embed.switch_space", json!("yes"), ConfigScope::Session)
    else {
        panic!("embed.switch_space must be a boolean");
    };
    assert_eq!(code, ErrorCode::InvalidType);

    // The width ceiling follows the storage engine (16000), not the old 4096 pin.
    let ceiling = crate::config::validate::MAX_EMBED_DIMS;
    assert_eq!(ceiling, 16_000);
    let ToolResult::Ok { .. } =
        rt_set.config_set("embed.dims", json!(ceiling), ConfigScope::Session)
    else {
        panic!("ceiling width settable");
    };
    let ToolResult::Err { code, .. } =
        rt_set.config_set("embed.dims", json!(ceiling + 1), ConfigScope::Session)
    else {
        panic!("above-ceiling width rejected");
    };
    assert_eq!(code, ErrorCode::OutOfRange);
}
