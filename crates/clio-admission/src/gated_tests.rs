// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//
//! # Responsibility
//!
//! Core gated-write path tests (gates, preview parity, backend failures).
//!
//! ## Owns
//! - Admit/reject wiring, preview parity, context validation, factor-unavailable
//!
//! ## Does not own
//! - Preview scrubbing / secret-mask telemetry cases (see `gated_telemetry_tests`)
//! - Tag-equality mismatch cases (see `gated_tag_tests`)
//!
//! ## Boundary
//!
//! Changes to gated_create / admit_preview behavior should normally require changes here.
//! Changes to preview masking should not require changes here.
//!
//! Keep this module focused on gated decision and write-path tests.

use super::gated_test_support::{
    StubSource, ctx, episodic_item, gate_req, gist_item, semantic_item, strong_source, weak_source,
};
use super::{VecSink, candidate_text, gated_create, gated_create_raw, signals_for};
use crate::decision::{Candidate, admit_preview};
use crate::policy::AdmissionPolicy;
use clio_config::ranking::RankingEnv;
use clio_types::{Actor, Bank, EpisodicType, ErrorCode, OpContext, SemanticCategory};

#[test]
fn candidate_text_combines_gist_and_snapshot() {
    let item = semantic_item("ct-1");
    let text = candidate_text(&item);
    assert!(text.contains("user prefers concise output"));
    assert!(text.contains("concise"));

    let mut bare = semantic_item("ct-2");
    bare.snapshot = None;
    bare.gist = None;
    assert_eq!(candidate_text(&bare), "");
}

#[test]
fn signals_come_from_the_item() {
    let item = semantic_item("sig-1");
    let signals = signals_for(&item, &strong_source()).expect("signals");
    assert_eq!(signals.text, candidate_text(&item));
    assert_eq!(signals.source_type, item.source_type);

    let down = StubSource {
        neighbors_down: true,
        ..StubSource::default()
    };
    let err = signals_for(&item, &down).expect_err("neighbors down");
    assert_eq!(err.code, ErrorCode::FactorUnavailable);
}

#[test]
fn missing_category_is_rejected_not_scored() {
    let env = RankingEnv::system_defaults();
    let policy = AdmissionPolicy::new(&env);
    let sink = VecSink::default();
    let ctx = ctx();
    let source = strong_source();

    // Semantic item with no category fails validation before scoring.
    let mut item = semantic_item("missing-cat");
    item.category = None;
    let req = gate_req(&item, &ctx, policy.clone(), &sink, "2026-09-17T00:00:00Z");
    let err = gated_create(
        &req,
        Candidate::Semantic(SemanticCategory::Persona),
        &source,
        &mut |_| panic!("invalid item must never write"),
    )
    .expect_err("missing category");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    let events = sink.events();
    assert!(events.is_empty(), "{events:?}");

    // Empty discriminator string is a logged gate reject, not a parse error.
    let item = semantic_item("empty-key");
    let req = gate_req(&item, &ctx, policy.clone(), &sink, "2026-09-17T00:00:00Z");
    let decision = gated_create_raw(&req, "semantic", "", &source, &mut |_| {
        panic!("empty key must never write")
    })
    .expect("reject, not error");
    assert!(!decision.pass);
    assert_eq!(sink.events().len(), 1);
}

#[test]
fn system_actor_is_gated_like_all_actors() {
    let env = RankingEnv::system_defaults();
    let policy = AdmissionPolicy::new(&env);
    let sink = VecSink::default();
    let sys = OpContext::new(Bank::new("bank-1"), Actor::System);

    // Strong candidate under the system actor still admits through the gates.
    let item = semantic_item("sys-strong");
    let source = strong_source();
    let req = gate_req(&item, &sys, policy.clone(), &sink, "2026-09-17T00:00:00Z");
    let decision = gated_create(
        &req,
        Candidate::Semantic(SemanticCategory::Persona),
        &source,
        &mut |_| Ok(()),
    )
    .expect("gated admit");
    assert!(decision.pass);

    // Weak candidate under the system actor still rejects: no actor bypass.
    let weak = gist_item("sys-weak", "hi");
    let req = gate_req(&weak, &sys, policy.clone(), &sink, "2026-09-17T00:00:00Z");
    let decision = gated_create(
        &req,
        Candidate::Episodic(EpisodicType::Gist),
        &weak_source(),
        &mut |_| panic!("reject must never write"),
    )
    .expect("reject");
    assert!(!decision.pass);
    assert_eq!(sink.events().len(), 2);
}

#[test]
fn illegal_category_and_type_are_logged_rejects() {
    let env = RankingEnv::system_defaults();
    let policy = AdmissionPolicy::new(&env);
    let sink = VecSink::default();
    let ctx = ctx();
    let item = semantic_item("cat-1");
    let source = strong_source();
    let req = gate_req(&item, &ctx, policy.clone(), &sink, "2026-09-17T00:00:00Z");
    let decision = gated_create_raw(&req, "semantic", "brain_dump", &source, &mut |_| {
        panic!("illegal category must never write")
    })
    .expect("reject, not error");
    assert!(!decision.pass);
    assert_eq!(sink.events().len(), 1);
    assert_eq!(sink.events()[0].operation, "admit_reject");

    let epi = episodic_item("epi-bad");
    let req = gate_req(&epi, &ctx, policy.clone(), &sink, "2026-09-17T00:00:00Z");
    let decision = gated_create_raw(&req, "episodic", "note", &source, &mut |_| {
        panic!("illegal type must never write")
    })
    .expect("reject, not error");
    assert!(!decision.pass);
    assert_eq!(sink.events().len(), 2);
}

#[test]
fn preview_matches_gated_write() {
    let env = RankingEnv::system_defaults();
    let policy = AdmissionPolicy::new(&env);
    let candidate = Candidate::Episodic(EpisodicType::Task);
    let item = episodic_item("prev-1");
    let source = strong_source();
    let signals = signals_for(&item, &source).expect("signals");
    let preview = admit_preview(candidate, &signals, &policy).expect("preview");

    let sink = VecSink::default();
    let ctx = ctx();
    let mut wrote = false;
    let req = gate_req(&item, &ctx, policy.clone(), &sink, "2026-09-17T00:00:00Z");
    let gated = gated_create(&req, candidate, &source, &mut |stored| {
        wrote = true;
        assert!(stored.admission_score.is_some());
        Ok(())
    })
    .expect("gated");
    assert_eq!(preview.pass, gated.pass);
    assert!(preview.pass);
    assert!((preview.admission_score - gated.admission_score).abs() < 1e-12);
    assert!(wrote);
    assert_eq!(sink.events().len(), 1);
}

#[test]
fn gated_write_validates_context_and_pairing() {
    let env = RankingEnv::system_defaults();
    let policy = AdmissionPolicy::new(&env);
    let sink = VecSink::default();
    let ctx = ctx();
    let source = strong_source();
    let item = semantic_item("pair-1");

    // Cross-kind admission is rejected as invalid input.
    let req = gate_req(&item, &ctx, policy.clone(), &sink, "t");
    let err = gated_create(
        &req,
        Candidate::Episodic(EpisodicType::Task),
        &source,
        &mut |_| Ok(()),
    )
    .expect_err("kind mismatch");
    assert_eq!(err.code, ErrorCode::InvalidArgument);

    // Empty bank is rejected.
    let bad_ctx = OpContext::new(Bank::new(""), Actor::Agent);
    let req = gate_req(&item, &bad_ctx, policy.clone(), &sink, "t");
    let err = gated_create(
        &req,
        Candidate::Semantic(SemanticCategory::Persona),
        &source,
        &mut |_| Ok(()),
    )
    .expect_err("empty bank");
    assert_eq!(err.code, ErrorCode::MissingBank);

    // Empty timestamp is rejected.
    let req = gate_req(&item, &ctx, policy.clone(), &sink, "");
    let err = gated_create(
        &req,
        Candidate::Semantic(SemanticCategory::Persona),
        &source,
        &mut |_| Ok(()),
    )
    .expect_err("empty timestamp");
    assert_eq!(err.code, ErrorCode::InvalidArgument);

    // Unknown kind string is an error, not a reject.
    let req = gate_req(&item, &ctx, policy.clone(), &sink, "t");
    let err = gated_create_raw(&req, "archive", "persona", &source, &mut |_| Ok(()))
        .expect_err("bad kind");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    let events = sink.events();
    assert!(events.is_empty(), "{events:?}");
}

#[test]
fn backend_failures_error_without_event_or_write() {
    let env = RankingEnv::system_defaults();
    let policy = AdmissionPolicy::new(&env);
    let sink = VecSink::default();
    let ctx = ctx();
    let item = semantic_item("down-1");

    let source = StubSource {
        neighbors_down: true,
        ..StubSource::default()
    };
    let req = gate_req(&item, &ctx, policy.clone(), &sink, "2026-09-17T00:00:00Z");
    let err = gated_create(
        &req,
        Candidate::Semantic(SemanticCategory::Persona),
        &source,
        &mut |_| panic!("backend down must never write"),
    )
    .expect_err("backend down");
    assert_eq!(err.code, ErrorCode::FactorUnavailable);
    let events = sink.events();
    assert!(events.is_empty(), "{events:?}");
}

#[test]
fn raw_semantic_admit_writes_once() {
    let env = RankingEnv::system_defaults();
    let policy = AdmissionPolicy::new(&env);
    let sink = VecSink::default();
    let ctx = ctx();
    let item = semantic_item("raw-sem");
    let source = strong_source();
    let mut writes = 0;
    let req = gate_req(&item, &ctx, policy.clone(), &sink, "2026-09-17T00:00:00Z");
    let decision = gated_create_raw(&req, "semantic", "persona", &source, &mut |stored| {
        writes += 1;
        assert!(stored.admission_score.is_some());
        Ok(())
    })
    .expect("raw semantic admit");
    assert!(decision.pass);
    assert_eq!(writes, 1);
    assert_eq!(sink.events()[0].operation, "admit_accept");
}

#[test]
fn write_failures_propagate_without_accept_event() {
    let env = RankingEnv::system_defaults();
    let policy = AdmissionPolicy::new(&env);
    let sink = VecSink::default();
    let ctx = ctx();
    let item = semantic_item("fail-1");
    let source = strong_source();
    let req = gate_req(&item, &ctx, policy.clone(), &sink, "2026-09-17T00:00:00Z");
    let err = gated_create(
        &req,
        Candidate::Semantic(SemanticCategory::Persona),
        &source,
        &mut |_| {
            Err(clio_types::AmError::new(
                ErrorCode::Internal,
                "storage down",
            ))
        },
    )
    .expect_err("write fails");
    assert_eq!(err.code, ErrorCode::Internal);
    let events = sink.events();
    assert!(events.is_empty(), "{events:?}");
}
