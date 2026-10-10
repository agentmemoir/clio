// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//
//! # Responsibility
//!
//! This module is responsible for tag-equality tests on the gated write path.
//!
//! ## Owns
//! - Same-kind tag-mismatch rejection tests (no threshold shopping)
//!
//! ## Does not own
//! - Scoring decisions (see `decision`)
//! - Gated writes and telemetry (see `gated`)
//!
//! ## Boundary
//!
//! Changes to tag-matching rules should normally require changes here.
//! Changes to scoring math should not require changes here.
//!
//! Keep this module focused on candidate/item tag-equality tests.

//! Tag-equality tests: candidate key must match the item tag (BUG-001).

use super::{GateRequest, SignalSource, VecSink, gated_create, gated_create_raw};
use crate::decision::Candidate;
use crate::policy::AdmissionPolicy;
use clio_config::ranking::RankingEnv;
use clio_types::{
    Actor, Bank, EpisodicType, EpistemicKind, ErrorCode, ItemKind, MemoryItem, OpContext,
    SemanticCategory,
};
use serde_json::json;

fn ctx() -> OpContext {
    OpContext::new(Bank::new("bank-1"), Actor::Agent)
}

fn item_with_tags(
    id: &str,
    category: Option<SemanticCategory>,
    tag: Option<EpisodicType>,
) -> MemoryItem {
    let (kind, category, episodic_type) = if category.is_some() {
        (ItemKind::Semantic, category, None)
    } else {
        (ItemKind::Episodic, None, tag)
    };
    MemoryItem {
        id: id.into(),
        bank_id: "bank-1".into(),
        subject_id: "subj".into(),
        kind,
        category,
        episodic_type,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        admission_score: None,
        source_ref: Some("turn_1".into()),
        snapshot: Some(json!({"style": "concise"})),
        gist: Some("user prefers concise output".into()),
        valid_from: "t0".into(),
        valid_until: None,
        tx_from: "t0".into(),
        tx_until: None,
        created_at: "t0".into(),
        updated_at: "t0".into(),
        context: None,
    }
}

/// Never-called source: tag mismatch must fail before any signal lookup.
#[derive(Debug)]
struct UnusedSource;

impl SignalSource for UnusedSource {
    fn neighbor_texts(&self, _item: &MemoryItem) -> Result<Vec<String>, clio_types::AmError> {
        panic!("tag mismatch must fail before signal lookup")
    }
}

#[test]
fn semantic_tag_mismatch_is_rejected_without_write_or_event() {
    let env = RankingEnv::system_defaults();
    let policy = AdmissionPolicy::new(&env);
    let sink = VecSink::default();
    let ctx = ctx();
    let item = item_with_tags("tag-mismatch-sem", Some(SemanticCategory::Schema), None);
    let req = GateRequest {
        item: &item,
        ctx: &ctx,
        policy,
        sink: &sink,
        timestamp: "2026-09-17T00:00:00Z",
    };
    let err = gated_create(
        &req,
        Candidate::Semantic(SemanticCategory::Persona),
        &UnusedSource,
        &mut |_| panic!("mismatch must never write"),
    )
    .expect_err("persona candidate with schema item");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    let events = sink.events();
    assert!(events.is_empty(), "{events:?}");
}

#[test]
fn episodic_tag_mismatch_is_rejected_without_write_or_event() {
    let env = RankingEnv::system_defaults();
    let policy = AdmissionPolicy::new(&env);
    let sink = VecSink::default();
    let ctx = ctx();
    let item = item_with_tags("tag-mismatch-epi", None, Some(EpisodicType::Task));
    let req = GateRequest {
        item: &item,
        ctx: &ctx,
        policy,
        sink: &sink,
        timestamp: "2026-09-17T00:00:00Z",
    };
    let err = gated_create(
        &req,
        Candidate::Episodic(EpisodicType::Gist),
        &UnusedSource,
        &mut |_| panic!("mismatch must never write"),
    )
    .expect_err("gist candidate with task item");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    let events = sink.events();
    assert!(events.is_empty(), "{events:?}");
}

#[test]
fn raw_mismatch_is_rejected_without_write_or_event() {
    let env = RankingEnv::system_defaults();
    let policy = AdmissionPolicy::new(&env);
    let sink = VecSink::default();
    let ctx = ctx();
    let item = item_with_tags("tag-mismatch-raw", Some(SemanticCategory::Schema), None);
    let req = GateRequest {
        item: &item,
        ctx: &ctx,
        policy,
        sink: &sink,
        timestamp: "2026-09-17T00:00:00Z",
    };
    let err = gated_create_raw(&req, "semantic", "persona", &UnusedSource, &mut |_| {
        panic!("mismatch must never write")
    })
    .expect_err("raw persona key with schema item");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    let events = sink.events();
    assert!(events.is_empty(), "{events:?}");
}
