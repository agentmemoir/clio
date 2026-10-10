// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the OpenAI adapter's
//! configuration-error messages, in particular that an operator is pointed at
//! the environment variable the binary reads.
//!
//! ## Owns
//! - Shape assertions on the adapter's fail-closed config messages
//!
//! ## Does not own
//! - Wire shape and response parsing (`openai_tests`)
//! - Provider selection (`provider`)
//!
//! ## Boundary
//!
//! Changes to the adapter's config-error messages should normally update tests
//! here. Changes to the request/response dialect should not require changes
//! here.
//!
//! Keep this module focused on the adapter's configuration messages.

use super::*;

#[test]
fn empty_url_error_names_the_prefixed_env_variable() {
    // The message tells an operator which variable to set, so it must name the
    // variable the binary actually reads. The assertion is on the name alone:
    // the sentence around it is free to change, the name is not.
    let err = OpenAiEmbedder::new("", "m", 8, None).expect_err("empty url");
    assert_eq!(err.code, ErrorCode::ConfigCorrupt);
    assert!(err.message.contains("CLIO_EMBED_URL"), "{err}");
}
