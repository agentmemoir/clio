// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the chat adapter's
//! configuration-error messages, in particular that an operator is pointed at
//! the environment variable the binary reads.
//!
//! ## Owns
//! - Shape assertions on `ChatExtractor::new` fail-closed config messages
//!
//! ## Does not own
//! - Prompt assembly and response parsing (`extract_chat_tests`,
//!   `extract_chat_egress_tests`, `extract_recall_tests`)
//! - Provider selection (`extract_factory`)
//!
//! ## Boundary
//!
//! Changes to the adapter's config-error messages should normally update tests
//! here. Changes to the request shape or validation rules should not require
//! changes here.
//!
//! Keep this module focused on the adapter's configuration messages.

use super::*;

#[derive(Debug)]
struct NoEgress;

impl Transport for NoEgress {
    fn post_json(&self, _url: &str, _body: &Value) -> Result<Value, AmError> {
        Err(AmError::new(ErrorCode::Internal, "no network in test"))
    }
}

#[test]
fn empty_url_error_names_the_prefixed_env_variable() {
    // The message tells an operator which variable to set, so it must name the
    // variable the binary actually reads. The assertion is on the name alone:
    // the sentence around it is free to change, the name is not.
    let err = ChatExtractor::new("", "m", 0.0, None, NoEgress).expect_err("no url");
    assert_eq!(err.code, ErrorCode::ConfigCorrupt);
    assert!(err.message.contains("CLIO_EXTRACT_URL"), "{err}");
}
