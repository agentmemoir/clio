// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the `extract.*` configuration knobs.
//!
//! ## Owns
//! - Defaults, environment wiring, allowlist, typed checks, secret masking, and
//!   bearer precedence for the extraction provider keys
//!
//! ## Does not own
//! - Production config assembly and validation (`validate`/`types`)
//! - Extraction adapter behavior (owned by the `clio-write` crate)
//!
//! ## Boundary
//!
//! Changes to extraction config-key semantics should normally update tests here.
//! Changes to extractor transport should not require changes here.
//!
//! Keep this module focused on extraction config-key behavior.

use crate::config::*;
use crate::error::{ErrorCode, ToolResult};
use serde_json::json;

#[test]
fn extract_provider_defaults_to_template() {
    let rt = Runtime::with_env(&|_| None);
    let eff = rt.effective_raw();
    assert_eq!(eff["extract"]["provider"], "template");
    assert_eq!(eff["extract"]["url"], "");
    assert_eq!(eff["extract"]["model"], "numind/NuExtract-1.5-tiny");
    assert_eq!(eff["extract"]["temperature"], 0.0);
}

#[test]
fn extract_env_knobs_reach_effective_config_and_mask_key() {
    let mut rt = Runtime::with_env(&|_| None);
    rt.apply_process_env(&mut |key| match key {
        "CLIO_EXTRACT_URL" => Some("https://api.example.com".into()),
        "CLIO_EXTRACT_MODEL" => Some("gpt-4o-mini".into()),
        "CLIO_EXTRACT_PROVIDER" => Some("openai".into()),
        "CLIO_EXTRACT_API_KEY" => Some("sk-extract-token-1234567890".into()),
        _ => None,
    });
    let eff = rt.effective_raw();
    assert_eq!(eff["extract"]["url"], "https://api.example.com");
    assert_eq!(eff["extract"]["model"], "gpt-4o-mini");
    assert_eq!(eff["extract"]["provider"], "openai");
    assert_eq!(
        eff["credentials"]["extract_api_key"],
        "sk-extract-token-1234567890"
    );
    let masked = serde_json::to_string(&rt.effective_masked()).unwrap();
    assert!(!masked.contains("sk-extract-token"));
    let ToolResult::Ok { data, .. } = rt.config_get(Some("credentials.extract_api_key")) else {
        panic!("config_get extract key");
    };
    let view = serde_json::to_string(&data.value).unwrap();
    assert!(view.contains("****") && !view.contains("sk-extract-token"));
}

#[test]
fn extract_paths_allowlist_and_typed_checks() {
    let mut rt = Runtime::with_env(&|_| None);
    rt.clear_deployment_path();
    let ToolResult::Ok { .. } = rt.config_profile_apply("coding_ci") else {
        panic!("apply profile");
    };
    for scope in [
        ConfigScope::Session,
        ConfigScope::Profile,
        ConfigScope::Deployment,
    ] {
        let ToolResult::Ok { .. } = rt.config_set("extract.provider", json!("openai"), scope)
        else {
            panic!("extract.provider settable scope={}", scope.as_str());
        };
        let ToolResult::Ok { .. } =
            rt.config_set("extract.url", json!("https://api.example.com"), scope)
        else {
            panic!("extract.url settable scope={}", scope.as_str());
        };
    }
    let ToolResult::Err { code, .. } =
        rt.config_set("extract.provider", json!("anthropic"), ConfigScope::Session)
    else {
        panic!("unknown extract.provider rejected");
    };
    assert_eq!(code, ErrorCode::OutOfRange);
    let ToolResult::Err { code, .. } =
        rt.config_set("extract.provider", json!(1), ConfigScope::Session)
    else {
        panic!("non-string extract.provider rejected");
    };
    assert_eq!(code, ErrorCode::InvalidType);
    let ToolResult::Err { code, .. } = rt.config_set(
        "credentials.extract_api_key",
        json!(7),
        ConfigScope::Session,
    ) else {
        panic!("non-string extract api key rejected");
    };
    assert_eq!(code, ErrorCode::InvalidType);
    // template (and empty) stay accepted so the local path remains default.
    let ToolResult::Ok { .. } =
        rt.config_set("extract.provider", json!("template"), ConfigScope::Session)
    else {
        panic!("template provider settable");
    };
    let ToolResult::Ok { .. } = rt.config_set("extract.provider", json!(""), ConfigScope::Session)
    else {
        panic!("empty extract.provider settable");
    };
}

#[test]
fn bearer_for_extract_precedence() {
    let mut rt = Runtime::with_env(&|_| None);
    assert_eq!(rt.bearer_for("extract"), None);
    rt.session_overlay
        .insert("credentials.api_key".into(), json!("generic-key-1"));
    assert_eq!(rt.bearer_for("extract"), Some("generic-key-1".to_owned()));
    rt.session_overlay
        .insert("credentials.extract_api_key".into(), json!("extract-key-1"));
    assert_eq!(rt.bearer_for("extract"), Some("extract-key-1".to_owned()));
    // Empty strings count as absent: fall back to the generic key.
    rt.session_overlay
        .insert("credentials.extract_api_key".into(), json!(""));
    assert_eq!(rt.bearer_for("extract"), Some("generic-key-1".to_owned()));
}

#[test]
fn extract_api_key_set_ack_is_masked() {
    let mut rt = Runtime::with_env(&|_| None);
    rt.clear_deployment_path();
    let ToolResult::Ok { data, .. } = rt.config_set(
        "credentials.extract_api_key",
        json!("sk-extract-token-9876543210"),
        ConfigScope::Session,
    ) else {
        panic!("extract api key set");
    };
    assert!(data.value.as_str().unwrap_or("").contains("****"));
    assert!(
        !data
            .value
            .as_str()
            .unwrap_or("")
            .contains("sk-extract-token")
    );
}
