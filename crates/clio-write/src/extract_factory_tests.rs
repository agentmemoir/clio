// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the extractor factory: provider
//! dispatch, fail-closed hosted config, and the no-egress default.
//!
//! ## Owns
//! - `build_extractor_from` dispatch for empty/`template`/`openai`/unknown
//!   providers, including the default-template no-egress path
//! - Fail-closed hosted config checks (missing url/model/key, bad scheme)
//!
//! ## Does not own
//! - Production factory logic under test (`extract_factory`)
//! - Chat adapter prompt/parse behavior (see `extract_chat_tests`)
//!
//! ## Boundary
//!
//! Changes to provider selection or the factory's fail-closed rules should
//! normally add or update tests here. Changes to an adapter's request shape
//! should not require changes here.
//!
//! Keep this module focused on factory provider dispatch.

use super::build_extractor_from;
use crate::extract::{ExtractRequest, Transport};
use clio_types::{AmError, ErrorCode};
use serde_json::{Value, json};

#[derive(Debug)]
struct FailingTransport;

impl Transport for FailingTransport {
    fn post_json(&self, _url: &str, _body: &Value) -> Result<Value, AmError> {
        Err(AmError::new(ErrorCode::Internal, "must not be called"))
    }
}

fn cfg(provider: &str, url: &str, model: &str) -> Value {
    json!({
        "extract": {"provider": provider, "url": url, "model": model, "temperature": 0.1}
    })
}

#[test]
fn t34_09_default_is_template_without_egress() {
    // Empty provider keeps the local template path; building the extractor
    // makes no network call (transport is only used on extract()).
    let ex = build_extractor_from(
        &cfg("", "", "numind/NuExtract-1.5-tiny"),
        None,
        FailingTransport,
    )
    .unwrap();
    assert!(format!("{ex:?}").contains("TemplateApiExtractor"));
    // template provider with a stub transport rides the same shape as before.
    let calls = std::sync::Arc::new(std::sync::Mutex::new(0u32));
    let ex = build_extractor_from(
        &cfg("template", "http://extract.example.com", "m"),
        None,
        Counting {
            calls: calls.clone(),
        },
    )
    .unwrap();
    ex.extract(&ExtractRequest {
        source_text: "Ada".into(),
        tool_payload: None,
        schema_template: json!({"entity": ""}),
        verify_feedback: None,
        context: None,
    })
    .unwrap();
    assert_eq!(*calls.lock().unwrap(), 1);
}

#[derive(Debug, Default)]
struct Counting {
    calls: std::sync::Arc<std::sync::Mutex<u32>>,
}

impl Transport for Counting {
    fn post_json(&self, _url: &str, _body: &Value) -> Result<Value, AmError> {
        *self.calls.lock().unwrap() += 1;
        Ok(
            json!({"choices": [{"message": {"content": "{\"snapshot\": {\"entity\": \"Ada\"}, \"gist\": \"g\"}"}}]}),
        )
    }
}

#[test]
fn t34_08_openai_factory_selects_chat_extractor_with_key() {
    let ex = build_extractor_from(
        &cfg("openai", "https://api.example.com", "gpt-test"),
        Some("sk-extract-token-1234567890".into()),
        Counting::default(),
    )
    .unwrap();
    let cand = ex
        .extract(&ExtractRequest {
            source_text: "Ada".into(),
            tool_payload: None,
            schema_template: json!({"entity": ""}),
            verify_feedback: None,
            context: None,
        })
        .unwrap();
    assert_eq!(cand.snapshot.unwrap()["entity"], "Ada");
    assert_eq!(cand.gist.as_deref(), Some("g"));
}

#[test]
fn t34_08_openai_factory_fails_closed_without_key_or_target() {
    // Missing key.
    let err = build_extractor_from(
        &cfg("openai", "https://api.example.com", "gpt-test"),
        None,
        FailingTransport,
    )
    .expect_err("no key");
    assert_eq!(err.code, ErrorCode::ConfigCorrupt, "{err}");
    assert!(!err.to_string().contains("sk-"), "{err}");
    // Missing url.
    let err = build_extractor_from(
        &cfg("openai", "", "gpt-test"),
        Some("k".into()),
        FailingTransport,
    )
    .expect_err("no url");
    assert_eq!(err.code, ErrorCode::ConfigCorrupt, "{err}");
    // Missing model.
    let err = build_extractor_from(
        &cfg("openai", "https://api.example.com", ""),
        Some("k".into()),
        FailingTransport,
    )
    .expect_err("no model");
    assert_eq!(err.code, ErrorCode::ConfigCorrupt, "{err}");
    // Bad endpoint scheme.
    let err = build_extractor_from(
        &cfg("openai", "ftp://api.example.com", "gpt-test"),
        Some("k".into()),
        FailingTransport,
    )
    .expect_err("bad scheme");
    assert_eq!(err.code, ErrorCode::ConfigCorrupt, "{err}");
}

#[test]
fn t34_07_unknown_provider_fails_closed() {
    let err = build_extractor_from(&cfg("anthropic", "u", "m"), None, FailingTransport)
        .expect_err("unknown provider");
    assert_eq!(err.code, ErrorCode::ConfigCorrupt, "{err}");
    assert!(err.to_string().contains("template|openai"), "{err}");
}

#[test]
fn t34_07_missing_provider_and_temperature_fall_back() {
    // A config without extract.provider/temperature at all keeps the
    // template default and a 0.0 temperature.
    let ex = build_extractor_from(&json!({}), None, Counting::default()).unwrap();
    let cand = ex
        .extract(&ExtractRequest {
            source_text: "Ada".into(),
            tool_payload: None,
            schema_template: json!({"entity": ""}),
            verify_feedback: None,
            context: None,
        })
        .unwrap();
    assert_eq!(cand.snapshot, None);
    assert_eq!(cand.source_ref, "extract");
}

#[test]
fn hosted_factory_rides_a_live_style_transport() {
    // The factory accepts the shared HttpTransport; endpoint validation is
    // the only offline-checkable behavior.
    let err = build_extractor_from(
        &cfg("openai", "ftp://api.example.com", "m"),
        Some("k".into()),
        crate::extract::HttpTransport::default(),
    )
    .expect_err("scheme rejected at factory time");
    assert_eq!(err.code, ErrorCode::ConfigCorrupt);
}
