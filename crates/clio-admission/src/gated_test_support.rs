// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//
//! # Responsibility
//!
//! Shared fixtures for gated-write tests (stubs, items, request builders).
//!
//! ## Owns
//! - Scripted `SignalSource` stubs and MemoryItem builders used by gated tests
//!
//! ## Does not own
//! - Gated write behavior or telemetry assertions
//!
//! ## Boundary
//!
//! Changes to fixture shape should normally require changes here.
//! Changes to admission decisions should not require changes here.
//!
//! Keep this module focused on test fixtures for the gated path.

use super::{GateRequest, SignalSource, VecSink, candidate_text};
use crate::factors::factor_unavailable;
use crate::policy::AdmissionPolicy;
use clio_types::{
    Actor, AmError, Bank, EpisodicType, EpistemicKind, ErrorCode, ItemKind, MemoryItem, OpContext,
    SemanticCategory,
};
use serde_json::json;

pub(super) fn ctx() -> OpContext {
    OpContext::new(Bank::new("bank-1"), Actor::Agent)
}

pub(super) fn gate_req<'a>(
    item: &'a MemoryItem,
    ctx: &'a OpContext,
    policy: AdmissionPolicy<'a>,
    sink: &'a VecSink,
    timestamp: &'a str,
) -> GateRequest<'a> {
    GateRequest {
        item,
        ctx,
        policy,
        sink,
        timestamp,
    }
}

/// Store stub: neighbors and priors are scripted; `*_down` fails the lookup.
#[derive(Debug, Default)]
pub(super) struct StubSource {
    pub(super) neighbors: Vec<String>,
    pub(super) mirror_item: bool,
    pub(super) neighbors_down: bool,
    pub(super) prior: Option<f64>,
    pub(super) prior_down: bool,
}

impl SignalSource for StubSource {
    fn neighbor_texts(&self, item: &MemoryItem) -> Result<Vec<String>, AmError> {
        if self.neighbors_down {
            return Err(factor_unavailable("novelty neighbor store"));
        }
        if self.mirror_item {
            return Ok(vec![candidate_text(item)]);
        }
        Ok(self.neighbors.clone())
    }

    fn prior_age_days(&self, _item: &MemoryItem) -> Result<Option<f64>, AmError> {
        if self.prior_down {
            return Err(AmError::new(ErrorCode::Internal, "recency store down"));
        }
        Ok(self.prior)
    }
}

pub(super) fn strong_source() -> StubSource {
    StubSource::default()
}

pub(super) fn weak_source() -> StubSource {
    StubSource {
        mirror_item: true,
        prior: Some(0.0),
        ..StubSource::default()
    }
}

pub(super) fn semantic_item(id: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: "bank-1".into(),
        subject_id: "subj".into(),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::Persona),
        episodic_type: None,
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

pub(super) fn gist_item(id: &str, gist: &str) -> MemoryItem {
    let mut item = semantic_item(id);
    item.kind = ItemKind::Episodic;
    item.category = None;
    item.episodic_type = Some(EpisodicType::Gist);
    item.snapshot = Some(json!({"memo": "1"}));
    item.gist = Some(gist.into());
    item
}

pub(super) fn episodic_item(id: &str) -> MemoryItem {
    let mut item = semantic_item(id);
    item.kind = ItemKind::Episodic;
    item.category = None;
    item.episodic_type = Some(EpisodicType::Task);
    item
}
