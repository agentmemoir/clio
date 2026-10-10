// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the verified store path: gate admission of a
//! previewed request, evidence-reference application, and the verify config
//! those paths share.
//!
//! ## Owns
//! - Behaviour assertions for `gated_store_verified`, `admit_preview_verified`,
//!   and `apply_evidence_ref`
//! - Rejection and error-path coverage for unverified store attempts
//!
//! ## Does not own
//! - The production gate logic (`store_path.rs`)
//! - Credential-label recording (`store_path_principal_tests`)
//!
//! ## Boundary
//!
//! Tests here break when the verified store path changes; they never change
//! it.
//!
//! Keep this module focused on the verified store path.

use super::store_path_test_support::{EmptyNeighbors, ctx, item_with_snapshot};
use crate::schema::default_event_fields;
use crate::store_path::{
    VerifiedStoreRequest, admit_preview_verified, apply_evidence_ref, gated_store_verified,
};
use crate::telemetry::VerifyVecSink;
use crate::verify::VerifyConfig;
use clio_admission::decision::Candidate;
use clio_admission::gated::candidate_text;
use clio_admission::{AdmissionPolicy, RankingEnv, VecSink};
use clio_types::{Actor, Bank, ErrorCode, MemoryItem, OpContext, SemanticCategory};
use serde_json::json;

#[test]
fn missing_bank_errors() {
    let ctx = OpContext::new(Bank::new(""), Actor::Agent);
    let env = RankingEnv::system_defaults();
    let policy = AdmissionPolicy::new(&env);
    let admit = VecSink::default();
    let verify_sink = VerifyVecSink::default();
    let fields = default_event_fields();
    let mut item = item_with_snapshot("i", json!({"entity": "Ada"}));
    item.snapshot = None;
    let mut req = VerifiedStoreRequest {
        item,
        ctx: &ctx,
        source_text: None,
        evidence_ref: None,
        fields: &fields,
        verify: VerifyConfig::default(),
        policy,
        admit_sink: &admit,
        verify_sink: &verify_sink,
        timestamp: "t0",
    };
    let err = gated_store_verified(
        &mut req,
        Candidate::Semantic(SemanticCategory::Persona),
        &EmptyNeighbors,
        &mut |_| Ok(()),
    )
    .expect_err("bank");
    assert_eq!(err.code, ErrorCode::MissingBank);
}

#[test]
fn empty_source_text_errors_like_missing() {
    let ctx = ctx();
    let env = RankingEnv::system_defaults();
    let policy = AdmissionPolicy::new(&env);
    let admit = VecSink::default();
    let verify_sink = VerifyVecSink::default();
    let fields = default_event_fields();
    let mut req = VerifiedStoreRequest {
        item: item_with_snapshot("i", json!({"entity": "Ada"})),
        ctx: &ctx,
        source_text: Some(""),
        evidence_ref: None,
        fields: &fields,
        verify: VerifyConfig::default(),
        policy,
        admit_sink: &admit,
        verify_sink: &verify_sink,
        timestamp: "t0",
    };
    let err = gated_store_verified(
        &mut req,
        Candidate::Semantic(SemanticCategory::Persona),
        &EmptyNeighbors,
        &mut |_| Ok(()),
    )
    .expect_err("empty source");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
}

#[test]
fn evidence_ref_maps_to_source_ref() {
    let mut item = item_with_snapshot("i", json!({"entity": "Ada"}));
    apply_evidence_ref(&mut item, Some("turn_99"));
    assert_eq!(item.source_ref.as_deref(), Some("turn_99"));
}

#[test]
fn t07_missing_source_text_is_structured_error() {
    let ctx = ctx();
    let env = RankingEnv::system_defaults();
    let policy = AdmissionPolicy::new(&env);
    let admit = VecSink::default();
    let verify_sink = VerifyVecSink::default();
    let fields = default_event_fields();
    let mut req = VerifiedStoreRequest {
        item: item_with_snapshot("i1", json!({"entity": "Ada"})),
        ctx: &ctx,
        source_text: None,
        evidence_ref: Some("turn_1"),
        fields: &fields,
        verify: VerifyConfig::default(),
        policy,
        admit_sink: &admit,
        verify_sink: &verify_sink,
        timestamp: "t0",
    };
    let mut wrote = false;
    let err = gated_store_verified(
        &mut req,
        Candidate::Semantic(SemanticCategory::Persona),
        &EmptyNeighbors,
        &mut |_| {
            wrote = true;
            Ok(())
        },
    )
    .expect_err("missing source");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert!(!wrote);
}

#[test]
fn verified_snapshot_admits_after_gates() {
    let ctx = ctx();
    let env = RankingEnv::system_defaults();
    let policy = AdmissionPolicy::new(&env);
    let admit = VecSink::default();
    let verify_sink = VerifyVecSink::default();
    let fields = default_event_fields();
    let source = "Ada likes concise style";
    let mut req = VerifiedStoreRequest {
        item: item_with_snapshot("i2", json!({"entity": "Ada"})),
        ctx: &ctx,
        source_text: Some(source),
        evidence_ref: Some("turn_2"),
        fields: &fields,
        verify: VerifyConfig::default(),
        policy,
        admit_sink: &admit,
        verify_sink: &verify_sink,
        timestamp: "t0",
    };
    let mut wrote = 0u32;
    let decision = gated_store_verified(
        &mut req,
        Candidate::Semantic(SemanticCategory::Persona),
        &EmptyNeighbors,
        &mut |_| {
            wrote += 1;
            Ok(())
        },
    )
    .expect("admit");
    assert!(decision.pass, "{decision:?}");
    assert_eq!(wrote, 1);
    assert_eq!(req.item.source_ref.as_deref(), Some("turn_2"));
    assert!(
        verify_sink
            .events()
            .iter()
            .any(|e| e.operation == "verify_ok")
    );
}

#[test]
fn t08_preview_unverified_no_write() {
    let ctx = ctx();
    let env = RankingEnv::system_defaults();
    let policy = AdmissionPolicy::new(&env);
    let verify_sink = VerifyVecSink::default();
    let fields = default_event_fields();
    let mut item = item_with_snapshot("i3", json!({"entity": "NOT_THERE"}));
    let decision = admit_preview_verified(
        &mut item,
        &ctx,
        Some("Ada is here"),
        Some("turn_3"),
        &fields,
        &VerifyConfig::default(),
        Candidate::Semantic(SemanticCategory::Persona),
        &EmptyNeighbors,
        &policy,
        &verify_sink,
        "t0",
    )
    .expect("preview");
    assert!(!decision.pass);
    assert!(
        decision
            .rejection_reason
            .as_deref()
            .unwrap_or("")
            .contains("span_verify")
    );
}

#[test]
fn t09_gist_only_skips_span_verify() {
    let ctx = ctx();
    let env = RankingEnv::system_defaults();
    let policy = AdmissionPolicy::new(&env);
    let admit = VecSink::default();
    let verify_sink = VerifyVecSink::default();
    let fields = default_event_fields();
    let mut item = item_with_snapshot("i4", json!({}));
    item.snapshot = None;
    item.gist = Some("prose only".into());
    // Avoid novelty self-match inflation: empty neighbors + unique text still admits.
    let _ = candidate_text(&item);
    let mut req = VerifiedStoreRequest {
        item,
        ctx: &ctx,
        source_text: None,
        evidence_ref: None,
        fields: &fields,
        verify: VerifyConfig::default(),
        policy,
        admit_sink: &admit,
        verify_sink: &verify_sink,
        timestamp: "t0",
    };
    let mut wrote = 0u32;
    let decision = gated_store_verified(
        &mut req,
        Candidate::Semantic(SemanticCategory::Persona),
        &EmptyNeighbors,
        &mut |_| {
            wrote += 1;
            Ok(())
        },
    )
    .expect("gist path");
    assert!(decision.pass, "{decision:?}");
    assert_eq!(wrote, 1);
    assert!(verify_sink.events().is_empty(), "no verify event");
}

#[test]
fn unverified_snapshot_never_writes() {
    let ctx = ctx();
    let env = RankingEnv::system_defaults();
    let policy = AdmissionPolicy::new(&env);
    let admit = VecSink::default();
    let verify_sink = VerifyVecSink::default();
    let fields = default_event_fields();
    let mut req = VerifiedStoreRequest {
        item: item_with_snapshot("i5", json!({"entity": "Ghost"})),
        ctx: &ctx,
        source_text: Some("Ada only"),
        evidence_ref: None,
        fields: &fields,
        verify: VerifyConfig::default(),
        policy,
        admit_sink: &admit,
        verify_sink: &verify_sink,
        timestamp: "t0",
    };
    let mut wrote = false;
    let decision = gated_store_verified(
        &mut req,
        Candidate::Semantic(SemanticCategory::Persona),
        &EmptyNeighbors,
        &mut |_| {
            wrote = true;
            Ok(())
        },
    )
    .expect("reject");
    assert!(!decision.pass);
    assert!(!wrote);
    assert!(admit.events().iter().any(|e| e.operation == "admit_reject"));
}

#[test]
fn context_does_not_satisfy_span_verification() {
    let ctx = ctx();
    let env = RankingEnv::system_defaults();
    let policy = AdmissionPolicy::new(&env);
    let admit = VecSink::default();
    let verify_sink = VerifyVecSink::default();
    let fields = default_event_fields();
    let mut item = item_with_snapshot("i6", json!({"entity": "Ada"}));
    // The haystack for is `source_text`; descriptive context may repeat the
    // value but must never count as the evidence.
    item.context = Some("Ada appears here".into());
    let mut req = VerifiedStoreRequest {
        item,
        ctx: &ctx,
        source_text: Some("a sentence with none of the values"),
        evidence_ref: None,
        fields: &fields,
        verify: VerifyConfig::default(),
        policy,
        admit_sink: &admit,
        verify_sink: &verify_sink,
        timestamp: "t0",
    };
    let mut wrote = false;
    let decision = gated_store_verified(
        &mut req,
        Candidate::Semantic(SemanticCategory::Persona),
        &EmptyNeighbors,
        &mut |_| {
            wrote = true;
            Ok(())
        },
    )
    .expect("reject");
    assert!(!decision.pass, "context must not satisfy span verify");
    assert!(
        decision
            .rejection_reason
            .as_deref()
            .unwrap_or("")
            .contains("span_verify")
    );
    assert!(!wrote);
}

#[test]
fn context_is_carried_on_the_admitted_item() {
    let ctx = ctx();
    let env = RankingEnv::system_defaults();
    let policy = AdmissionPolicy::new(&env);
    let admit = VecSink::default();
    let verify_sink = VerifyVecSink::default();
    let fields = default_event_fields();
    let mut item = item_with_snapshot("i7", json!({"entity": "Ada"}));
    item.context = Some("team chat".into());
    let mut req = VerifiedStoreRequest {
        item,
        ctx: &ctx,
        source_text: Some("Ada likes concise style"),
        evidence_ref: None,
        fields: &fields,
        verify: VerifyConfig::default(),
        policy,
        admit_sink: &admit,
        verify_sink: &verify_sink,
        timestamp: "t0",
    };
    let mut written: Option<MemoryItem> = None;
    let decision = gated_store_verified(
        &mut req,
        Candidate::Semantic(SemanticCategory::Persona),
        &EmptyNeighbors,
        &mut |item| {
            written = Some(item.clone());
            Ok(())
        },
    )
    .expect("admit");
    assert!(decision.pass, "{decision:?}");
    assert_eq!(req.item.context.as_deref(), Some("team chat"));
    assert_eq!(
        written.expect("written item").context.as_deref(),
        Some("team chat")
    );
}
