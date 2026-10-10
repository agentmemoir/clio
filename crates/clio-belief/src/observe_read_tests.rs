// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//! Read responses surface epistemic_kind and belief metadata.

use crate::observe::{BeliefObserveArgs, BeliefObserveOutcome, BeliefVecSink, belief_observe};
use crate::signals::BeliefSignalSource;
use clio_admission::{AdmissionPolicy, RankingEnv};
use clio_store::{DekProvider, LocalDevKms, SqliteStore, Store};
use clio_types::{
    Actor, AmError, Bank, EpisodicType, EpistemicKind, ItemKind, MemoryItem, OpContext,
    RetrieveHit, SemanticCategory, SourceType,
};
use std::sync::Arc;

#[derive(Debug, Default)]
struct EmptySignals;

impl BeliefSignalSource for EmptySignals {
    fn neighbor_texts(&self, _bank_id: &str, _proposition: &str) -> Result<Vec<String>, AmError> {
        Ok(vec![])
    }
}

fn item(bank: &str, id: &str, kind: ItemKind) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: "subject-1".into(),
        kind,
        category: match kind {
            ItemKind::Semantic => Some(SemanticCategory::Persona),
            ItemKind::Episodic => None,
        },
        episodic_type: match kind {
            ItemKind::Semantic => None,
            ItemKind::Episodic => Some(EpisodicType::Triple),
        },
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        admission_score: Some(0.9),
        source_ref: None,
        snapshot: None,
        gist: Some("prose".into()),
        valid_from: "2026-01-01".into(),
        valid_until: None,
        tx_from: "2026-01-01".into(),
        tx_until: None,
        created_at: "2026-01-01".into(),
        updated_at: "2026-01-01".into(),
        context: None,
    }
}

#[test]
fn fact_item_hit_surfaces_kind_without_trajectory() {
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    let store = SqliteStore::open_in_memory(kms).unwrap();
    let fact = item("bank-read", "fact-1", ItemKind::Semantic);
    store.create_memory_item(&fact, "agent").unwrap();
    let got = store
        .get_memory_item("bank-read", "fact-1")
        .unwrap()
        .unwrap();
    assert_eq!(got.epistemic_kind, EpistemicKind::Fact);
    let hit = RetrieveHit::from_item(&got);
    assert_eq!(hit.epistemic_kind, EpistemicKind::Fact);
    assert!(hit.source_type.is_none());
    assert!(hit.confidence.is_none());
    assert!(!hit.requires_confidence_check);
    let json = serde_json::to_string(&hit).unwrap();
    assert!(json.contains("\"epistemic_kind\":\"fact\""));
    assert!(!json.contains("\"source_type\""));
    assert!(!json.contains("\"confidence\""));
}

#[test]
fn belief_item_hit_surfaces_source_type_and_confidence() {
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    let store = SqliteStore::open_in_memory(kms).unwrap();
    let mut belief = item("bank-read", "belief-item-1", ItemKind::Episodic);
    belief.epistemic_kind = EpistemicKind::Belief;
    belief.source_type = Some(SourceType::UserStated);
    belief.confidence = Some(0.8);
    store.create_memory_item(&belief, "agent").unwrap();
    let got = store
        .get_memory_item("bank-read", "belief-item-1")
        .unwrap()
        .unwrap();
    let hit = RetrieveHit::from_item(&got);
    assert_eq!(hit.epistemic_kind, EpistemicKind::Belief);
    assert_eq!(hit.source_type, Some(SourceType::UserStated));
    assert_eq!(hit.confidence, Some(0.8));
    assert!(hit.requires_confidence_check);
    let json = serde_json::to_string(&hit).unwrap();
    assert!(json.contains("\"epistemic_kind\":\"belief\""));
    assert!(json.contains("\"source_type\":\"user_stated\""));
    assert!(json.contains("\"confidence\":0.8"));
}

#[test]
fn belief_object_hit_reflects_current_history_entry() {
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    let store = SqliteStore::open_in_memory(kms).unwrap();
    let ctx = OpContext::new(Bank::new("bank-read"), Actor::Agent);
    let sink = BeliefVecSink::default();
    let env = RankingEnv::default();
    let out = belief_observe(
        &store,
        &EmptySignals,
        &BeliefObserveArgs {
            context: None,
            ctx: &ctx,
            subject_id: "subject-1",
            proposition: "user likes coffee",
            confidence: 0.4,
            evidence_ref: "turn_1",
            source_type: "agent_inferred",
            as_of: Some("2026-06-01"),
            policy: AdmissionPolicy::new(&env),
            sink: &sink,
            timestamp: "2026-06-01T00:00:00Z",
        },
    )
    .unwrap();
    let BeliefObserveOutcome::Created { belief, .. } = out else {
        panic!("create");
    };
    let hit = RetrieveHit::from_belief(&belief);
    assert_eq!(hit.kind, "belief");
    assert_eq!(hit.epistemic_kind, EpistemicKind::Belief);
    assert_eq!(hit.source_type, Some(SourceType::AgentInferred));
    assert_eq!(hit.confidence, Some(0.4));
    assert_eq!(hit.gist.as_deref(), Some("user likes coffee"));
    assert!(hit.requires_confidence_check);
}
