// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//
//! # Responsibility
//!
//! This module is responsible for store-path tests of the unlisted-snapshot-leaf
//! scope rule at the gated-store boundary.
//!
//! ## Owns
//! - Proof that a rejected unlisted leaf writes nothing
//! - Proof that the rejection surface carries no caller-supplied snapshot key
//!
//! ## Does not own
//! - Verifier-internal leaf walking and reporting (see `verify`)
//! - Other store-path gate ordering tests (see `store_path_tests`)
//!
//! ## Boundary
//!
//! Changes to the unlisted-leaf rejection surface should normally require
//! changes here. Changes to unrelated store-path gating should not.

use crate::schema::default_event_fields;
use crate::store_path::{VerifiedStoreRequest, gated_store_verified};
use crate::telemetry::VerifyVecSink;
use crate::verify::VerifyConfig;
use clio_admission::decision::Candidate;
use clio_admission::gated::SignalSource;
use clio_admission::{AdmissionPolicy, RankingEnv, VecSink};
use clio_types::{
    Actor, AmError, Bank, EpistemicKind, ItemKind, MemoryItem, OpContext, SemanticCategory,
};
use serde_json::json;

#[derive(Debug, Default)]
struct EmptyNeighbors;

impl SignalSource for EmptyNeighbors {
    fn neighbor_texts(&self, _item: &MemoryItem) -> Result<Vec<String>, AmError> {
        Ok(vec![])
    }
}

fn item_with_snapshot(id: &str, snap: serde_json::Value) -> MemoryItem {
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
        source_ref: None,
        snapshot: Some(snap),
        gist: Some("gist".into()),
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
fn unlisted_leaf_rejection_carries_no_caller_key() {
    // The offending snapshot key is caller-controlled and may itself be the
    // secret, so the whole rejection surface must stay free of it: the returned
    // decision, the verify event, and the admission event.
    let ctx = OpContext::new(Bank::new("bank-1"), Actor::Agent);
    let env = RankingEnv::system_defaults();
    let policy = AdmissionPolicy::new(&env);
    let admit = VecSink::default();
    let verify_sink = VerifyVecSink::default();
    let fields = default_event_fields();
    let secret_key = "sk-live-LEAKEDKEY99";
    let mut req = VerifiedStoreRequest {
        item: item_with_snapshot("i8", json!({"entity": "Ada", secret_key: "x"})),
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
    assert!(!wrote, "a rejected snapshot must not be written");

    // The rejection still names the rule and a bounded position label.
    let reason = decision.rejection_reason.expect("rejection reason");
    assert!(reason.contains("span_verify"), "{reason}");
    assert!(
        reason.contains("unlisted snapshot leaf rejected"),
        "{reason}"
    );
    assert!(reason.contains("<unlisted leaf #0>"), "{reason}");

    // Telemetry: the failure label is the only caller-derived text these two
    // events carry, and it must be the placeholder rather than the key.
    let verify_events = verify_sink.events();
    let fail = verify_events
        .iter()
        .find(|e| e.operation == "verify_fail")
        .expect("verify_fail event");
    assert_eq!(fail.failure_fields, vec!["<unlisted leaf #0>".to_owned()]);
    let admit_events = admit.events();
    let reject = admit_events
        .iter()
        .find(|e| e.operation == "admit_reject")
        .expect("admit_reject event");
    let reject_reason = reject.rejection_reason.clone().expect("rejection reason");
    for (sink_name, text) in [("decision", &reason), ("admission", &reject_reason)] {
        assert!(
            !text.contains(secret_key),
            "{sink_name} leaked the key: {text}"
        );
        assert!(
            !text.contains("sk-live"),
            "{sink_name} leaked a key fragment: {text}"
        );
    }
}
