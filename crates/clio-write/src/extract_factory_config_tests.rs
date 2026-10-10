// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the extractor factory's
//! configuration-error messages, in particular that an operator is pointed at
//! the environment variable the binary reads.
//!
//! ## Owns
//! - Shape assertions on the factory's missing-key config message
//!
//! ## Does not own
//! - Provider dispatch and the no-egress default (`extract_factory_tests`)
//! - Chat adapter prompt/parse behavior (`extract_chat_tests`)
//!
//! ## Boundary
//!
//! Changes to the factory's fail-closed config messages should normally update
//! tests here. Changes to provider selection should not require changes here.
//!
//! Keep this module focused on the factory's configuration messages.

use super::*;
use serde_json::json;

#[derive(Debug)]
struct NoEgress;

impl Transport for NoEgress {
    fn post_json(&self, _url: &str, _body: &Value) -> Result<Value, AmError> {
        Err(AmError::new(ErrorCode::Internal, "no network in test"))
    }
}

#[test]
fn missing_key_error_names_the_prefixed_env_variable() {
    // The message tells an operator which variable to set, so it must name the
    // variable the binary actually reads. The assertion is on the name alone:
    // the sentence around it is free to change, the name is not. No key value
    // is written here, and none may appear in the message.
    let effective = json!({
        "extract": {"provider": "openai", "url": "https://api.example.com", "model": "m"}
    });
    let err = build_extractor_from(&effective, None, NoEgress).expect_err("no key");
    assert_eq!(err.code, ErrorCode::ConfigCorrupt);
    assert!(err.message.contains("CLIO_EXTRACT_API_KEY"), "{err}");
}
