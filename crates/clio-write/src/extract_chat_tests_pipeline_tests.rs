// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the pipeline-level coverage of the hosted
//! chat extraction adapter: malformed model content must refuse the commit,
//! and an ungrounded snapshot must be refused by span verification.
//!
//! ## Owns
//! - `extract_and_verify` integration over the chat adapter (malformed and
//!   ungrounded variants)
//!
//! ## Does not own
//! - Prompt assembly and response parsing (`extract_chat_tests`)
//! - Egress masking and provider-error surfacing (`extract_chat_egress_tests`)
//! - The verify pipeline itself (`pipeline`, `verify`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the chat adapter's pipeline integration.

use super::{StubTransport, chat_response};

use crate::extract_chat::{ChatExtractor, parse_chat_response};
use clio_types::{AmError, ErrorCode};
use serde_json::json;

#[test]
fn t34_02_pipeline_refuses_commit_on_malformed_content() {
    use crate::pipeline::{ExtractVerifyArgs, extract_and_verify};
    use crate::schema::default_event_fields;
    use crate::verify::VerifyConfig;

    #[derive(Debug)]
    struct BadJsonChat;
    impl crate::extract::Extractor for BadJsonChat {
        fn extract(
            &self,
            _req: &crate::extract::ExtractRequest,
        ) -> Result<crate::extract::ExtractCandidate, AmError> {
            parse_chat_response(&chat_response("not json at all"))
        }
    }
    let verify = VerifyConfig::default();
    let out = extract_and_verify(ExtractVerifyArgs {
        context: None,
        extractor: &BadJsonChat,
        source_text: "Ada was here",
        schema_template: json!({"entity": ""}),
        tool_payload: None,
        fields: &default_event_fields(),
        verify: &verify,
        emit: None,
    })
    .expect_err("propagates");
    assert_eq!(out.code, ErrorCode::Internal);
}

#[test]
fn t34_11_ungrounded_snapshot_is_refused_by_span_verify() {
    use crate::pipeline::{ExtractVerifyArgs, extract_and_verify};
    use crate::schema::default_event_fields;
    use crate::verify::VerifyConfig;

    let ex = ChatExtractor::new(
        "https://api.example.com",
        "m",
        0.0,
        None,
        StubTransport {
            reply: chat_response(
                "{\"snapshot\": {\"entity\": \"PARAPHRASE_NOT_IN_SOURCE\"}, \"gist\": \"gist\"}",
            ),
        },
    )
    .unwrap();
    let verify = VerifyConfig::default();
    let out = extract_and_verify(ExtractVerifyArgs {
        context: None,
        extractor: &ex,
        source_text: "Ada was here",
        schema_template: json!({"entity": ""}),
        tool_payload: None,
        fields: &default_event_fields(),
        verify: &verify,
        emit: None,
    })
    .unwrap();
    assert!(out.candidate.is_none());
    let retained = out.retained.expect("non-authoritative retention");
    assert_eq!(retained.gist.as_deref(), Some("gist"));
}
