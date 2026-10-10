// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for negative ingest-gate tests (admission reject,
//! cross-bank isolation).
//!
//! ## Owns
//! - admission-reject and cross-bank test fixtures and assertions
//!
//! ## Does not own
//! - Production ingest behavior or admission scoring math
//!
//! ## Boundary
//!
//! Changes to ingest gate tests should normally require changes here.
//! Changes to admission policy should not require changes here.
//!
//! Keep this module focused on negative ingest-gate coverage.

use crate::chunk::Turn;
use crate::extract::{ExtractCandidate, FixtureExtractor};
use crate::ingest::{ParallelIngestArgs, WriteIngestConfig, parallel_ingest};
use crate::maintenance::{LeafIndex, NoopMaintenance};
use crate::schema::default_event_fields;
use crate::telemetry::VerifyVecSink;
use crate::verify::VerifyConfig;
use clio_admission::decision::Candidate;
use clio_admission::gated::{SignalSource, candidate_text};
use clio_admission::{AdmissionPolicy, RankingEnv, VecSink};
use clio_types::{
    Actor, AmError, Bank, EpisodicType, EpistemicKind, ErrorCode, ItemKind, MemoryItem, OpContext,
};
use serde_json::json;
use std::sync::Arc;

/// Neighbor source that mirrors the candidate text (novelty = 0), for reject tests.
#[derive(Debug, Default)]
struct MirrorNeighbors;

impl SignalSource for MirrorNeighbors {
    fn neighbor_texts(&self, item: &MemoryItem) -> Result<Vec<String>, AmError> {
        Ok(vec![candidate_text(item)])
    }
    fn prior_age_days(&self, _item: &MemoryItem) -> Result<Option<f64>, AmError> {
        Ok(None)
    }
}

fn base_item(bank: &str) -> MemoryItem {
    MemoryItem {
        id: "leaf".into(),
        bank_id: bank.into(),
        subject_id: "subj".into(),
        kind: ItemKind::Episodic,
        category: None,
        episodic_type: Some(EpisodicType::Temporal),
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        admission_score: None,
        source_ref: None,
        snapshot: None,
        gist: None,
        valid_from: "t0".into(),
        valid_until: None,
        tx_from: "t0".into(),
        tx_until: None,
        created_at: "t0".into(),
        updated_at: "t0".into(),
        context: None,
    }
}

#[test]
fn t08_rejected_by_admission_gate() {
    // Structurally valid + span-verified candidate, but admission scoring rejects
    // (mirrored neighbors → novelty 0; theta pinned at 1.0). The gate must stop it
    // before write and before any leaf publish.
    let mut fx = FixtureExtractor::new();
    fx.insert(
        "Ada",
        ExtractCandidate {
            snapshot: Some(json!({"entity": "Ada"})),
            gist: Some("g".into()),
            source_ref: "t".into(),
            diagnostic: None,
        },
    );
    let ctx = OpContext::new(Bank::new("bank-1"), Actor::Agent);
    let mut env = RankingEnv::system_defaults();
    env.theta_admit = 1.0;
    let admit = VecSink::default();
    let verify_sink = VerifyVecSink::default();
    let fields = default_event_fields();
    let leaves = LeafIndex::new();
    let mut wrote = false;
    let out = parallel_ingest(
        &ParallelIngestArgs {
            extractor: &fx,
            turns: &[Turn::text("Ada")],
            schema_template: json!({"entity": ""}),
            fields: &fields,
            verify: &VerifyConfig::default(),
            item_base: base_item("bank-1"),
            candidate: Candidate::Episodic(EpisodicType::Temporal),
            signal_source: &MirrorNeighbors,
            ctx: &ctx,
            policy: AdmissionPolicy::new(&env),
            admit_sink: &admit,
            verify_sink: &verify_sink,
            timestamp: "t0",
            config: WriteIngestConfig::default(),
            leaves: &leaves,
            maintenance: Arc::new(NoopMaintenance),
        },
        &mut |_| {
            wrote = true;
            Ok(())
        },
    )
    .expect("ingest");
    assert!(!wrote, "write ran despite admission reject");
    assert!(out.admitted.is_empty(), "admitted: {:?}", out.admitted);
    assert!(!out.leaf_queryable);
    let published = leaves.list_leaves("bank-1");
    assert!(published.is_empty(), "published: {published:?}");
    assert!(
        admit.events().iter().any(|e| e.operation == "admit_reject"),
        "no admission reject event recorded"
    );
}

#[test]
fn t09_cross_bank_rejected() {
    // `item_base` bank disagrees with the op context: the guard rejects before any
    // chunk runs, with no write and no leaf publish in either bank.
    let ctx = OpContext::new(Bank::new("bank-1"), Actor::Agent);
    let env = RankingEnv::system_defaults();
    let admit = VecSink::default();
    let verify_sink = VerifyVecSink::default();
    let fields = default_event_fields();
    let leaves = LeafIndex::new();
    let fx = FixtureExtractor::new();
    let mut wrote = false;
    let err = parallel_ingest(
        &ParallelIngestArgs {
            extractor: &fx,
            turns: &[Turn::text("Ada")],
            schema_template: json!({}),
            fields: &fields,
            verify: &VerifyConfig::default(),
            item_base: base_item("other-bank"),
            candidate: Candidate::Episodic(EpisodicType::Temporal),
            signal_source: &MirrorNeighbors,
            ctx: &ctx,
            policy: AdmissionPolicy::new(&env),
            admit_sink: &admit,
            verify_sink: &verify_sink,
            timestamp: "t0",
            config: WriteIngestConfig::default(),
            leaves: &leaves,
            maintenance: Arc::new(NoopMaintenance),
        },
        &mut |_| {
            wrote = true;
            Ok(())
        },
    )
    .expect_err("cross-bank");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert!(!wrote);
    let own_leaves = leaves.list_leaves("bank-1");
    assert!(own_leaves.is_empty(), "leaves: {own_leaves:?}");
    let other_leaves = leaves.list_leaves("other-bank");
    assert!(other_leaves.is_empty(), "leaves: {other_leaves:?}");
}
