// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0

use crate::chunk::Turn;
use crate::extract::{
    ExtractCandidate, ExtractRequest, Extractor, FixtureExtractor, PoisonExtractor,
};
use crate::ingest::{DEFAULT_CONCURRENCY, ParallelIngestArgs, WriteIngestConfig, parallel_ingest};
use crate::maintenance::{LeafIndex, NoopMaintenance, SharedMaintenanceHook};
use crate::schema::{ExtractiveKind, FieldSpec, default_event_fields};
use crate::telemetry::VerifyVecSink;
use crate::verify::VerifyConfig;
use clio_admission::decision::Candidate;
use clio_admission::gated::SignalSource;
use clio_admission::{AdmissionPolicy, RankingEnv, VecSink};
use clio_types::{
    Actor, AmError, Bank, EpisodicType, EpistemicKind, ItemKind, MemoryItem, OpContext,
};
use serde_json::json;
use std::sync::Arc;

#[derive(Debug, Default)]
struct EmptyNeighbors;

impl SignalSource for EmptyNeighbors {
    fn neighbor_texts(&self, _item: &MemoryItem) -> Result<Vec<String>, AmError> {
        Ok(vec![])
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

fn run_ingest(
    extractor: &(dyn Extractor + Sync),
    turns: &[Turn],
    config: WriteIngestConfig,
    leaves: &LeafIndex,
    maintenance: SharedMaintenanceHook,
    fields: &[FieldSpec],
    write: &mut dyn FnMut(&MemoryItem) -> Result<(), AmError>,
) -> crate::ingest::IngestOutcome {
    let ctx = OpContext::new(Bank::new("bank-1"), Actor::Agent);
    let env = RankingEnv::system_defaults();
    let admit = VecSink::default();
    let verify_sink = VerifyVecSink::default();
    parallel_ingest(
        &ParallelIngestArgs {
            extractor,
            turns,
            schema_template: json!({"entity": ""}),
            fields,
            verify: &VerifyConfig::default(),
            item_base: base_item("bank-1"),
            candidate: Candidate::Episodic(EpisodicType::Temporal),
            signal_source: &EmptyNeighbors,
            ctx: &ctx,
            policy: AdmissionPolicy::new(&env),
            admit_sink: &admit,
            verify_sink: &verify_sink,
            timestamp: "t0",
            config,
            leaves,
            maintenance,
        },
        write,
    )
    .expect("ingest")
}

#[test]
fn t02_near_duplicates_one_leaf() {
    let mut fx = FixtureExtractor::new();
    fx.insert(
        "Ada\nAda",
        ExtractCandidate {
            snapshot: Some(json!({"entity": "Ada"})),
            gist: Some("g1".into()),
            source_ref: "a".into(),
            diagnostic: None,
        },
    );
    // With max_turns=1 each turn is its own chunk
    fx.insert(
        "Ada",
        ExtractCandidate {
            snapshot: Some(json!({"entity": "Ada"})),
            gist: Some("g2".into()),
            source_ref: "b".into(),
            diagnostic: None,
        },
    );
    let turns = vec![Turn::text("Ada"), Turn::text("Ada")];
    let leaves = LeafIndex::new();
    let mut wrote = 0u32;
    let out = run_ingest(
        &fx,
        &turns,
        WriteIngestConfig {
            max_turns_per_chunk: 1,
            ..WriteIngestConfig::default()
        },
        &leaves,
        Arc::new(NoopMaintenance),
        &default_event_fields(),
        &mut |_| {
            wrote += 1;
            Ok(())
        },
    );
    assert_eq!(out.admitted.len(), 1, "{out:?}");
    assert_eq!(wrote, 1);
    assert!(out.consolidate_merges >= 1);
    assert!(out.leaf_queryable);
    let id = &out.admitted[0].item_id;
    assert!(leaves.get("bank-1", id).is_some());
}

#[test]
fn t03_contradiction_two_units() {
    let mut fx = FixtureExtractor::new();
    fx.insert(
        "Alice lives in Boston",
        ExtractCandidate {
            snapshot: Some(json!({
                "entity": "Alice",
                "subject": "Alice",
                "predicate": "lives in",
                "object": "Boston"
            })),
            gist: None,
            source_ref: "b".into(),
            diagnostic: None,
        },
    );
    fx.insert(
        "Alice lives in NYC",
        ExtractCandidate {
            snapshot: Some(json!({
                "entity": "Alice",
                "subject": "Alice",
                "predicate": "lives in",
                "object": "NYC"
            })),
            gist: None,
            source_ref: "n".into(),
            diagnostic: None,
        },
    );
    let turns = vec![
        Turn::text("Alice lives in Boston"),
        Turn::text("Alice lives in NYC"),
    ];
    let leaves = LeafIndex::new();
    let mut wrote = 0u32;
    // The SPO-shaped fixture is valid only when the deployment declares the
    // SPO fields; production `default_event_fields` refuses them (see the
    // unlisted-leaf refusal test below).
    let fields = vec![
        FieldSpec::new("entity", ExtractiveKind::Entity, true),
        FieldSpec::new("subject", ExtractiveKind::Entity, false),
        FieldSpec::new("predicate", ExtractiveKind::Entity, false),
        FieldSpec::new("object", ExtractiveKind::Entity, false),
    ];
    let out = run_ingest(
        &fx,
        &turns,
        WriteIngestConfig {
            max_turns_per_chunk: 1,
            ..WriteIngestConfig::default()
        },
        &leaves,
        Arc::new(NoopMaintenance),
        &fields,
        &mut |_| {
            wrote += 1;
            Ok(())
        },
    );
    assert_eq!(out.admitted.len(), 2);
    assert_eq!(wrote, 2);
    assert_eq!(out.contradiction_splits, 1);
}

#[test]
fn spo_candidates_refused_under_default_fields() {
    let mut fx = FixtureExtractor::new();
    fx.insert(
        "Alice lives in Boston",
        ExtractCandidate {
            snapshot: Some(json!({
                "entity": "Alice",
                "subject": "Alice",
                "predicate": "lives in",
                "object": "Boston"
            })),
            gist: None,
            source_ref: "b".into(),
            diagnostic: None,
        },
    );
    let turns = vec![Turn::text("Alice lives in Boston")];
    let leaves = LeafIndex::new();
    let out = run_ingest(
        &fx,
        &turns,
        WriteIngestConfig {
            max_turns_per_chunk: 1,
            ..WriteIngestConfig::default()
        },
        &leaves,
        Arc::new(NoopMaintenance),
        &default_event_fields(),
        &mut |_| Ok(()),
    );
    // The declared `entity` grounds, but `subject`/`predicate`/`object` are
    // unlisted leaves: the snapshot is refused, not admitted.
    assert!(out.admitted.is_empty(), "{out:?}");
}

#[test]
fn t04_partial_chunk_failure() {
    #[derive(Debug)]
    struct Mixed {
        poison: PoisonExtractor,
        good: FixtureExtractor,
    }
    impl Extractor for Mixed {
        fn extract(&self, req: &ExtractRequest) -> Result<ExtractCandidate, AmError> {
            if req.source_text.contains("BAD") {
                self.poison.extract(req)
            } else {
                self.good.extract(req)
            }
        }
    }
    let mut good = FixtureExtractor::new();
    good.insert(
        "Ada ok",
        ExtractCandidate {
            snapshot: Some(json!({"entity": "Ada"})),
            gist: Some("g".into()),
            source_ref: "ok".into(),
            diagnostic: None,
        },
    );
    let fx = Mixed {
        poison: PoisonExtractor::new("bad"),
        good,
    };
    let turns = vec![Turn::text("Ada ok"), Turn::text("BAD text")];
    let leaves = LeafIndex::new();
    let mut wrote = 0u32;
    let out = run_ingest(
        &fx,
        &turns,
        WriteIngestConfig {
            max_turns_per_chunk: 1,
            all_or_nothing: false,
            ..WriteIngestConfig::default()
        },
        &leaves,
        Arc::new(NoopMaintenance),
        &default_event_fields(),
        &mut |_| {
            wrote += 1;
            Ok(())
        },
    );
    assert_eq!(wrote, 1);
    assert_eq!(out.admitted.len(), 1);
    assert!(!out.retained.is_empty(), "{:?}", out.retained);
}

#[test]
fn profile_configs_are_distinct() {
    assert_eq!(WriteIngestConfig::for_profile("A").concurrency, 1);
    assert_eq!(WriteIngestConfig::for_profile("B").concurrency, 2);
    assert_eq!(WriteIngestConfig::for_profile("C").concurrency, 4);
    assert_eq!(
        WriteIngestConfig::for_profile("custom").concurrency,
        DEFAULT_CONCURRENCY
    );
    assert_eq!(
        WriteIngestConfig::for_profile("local-rft").profile_label,
        "local-rft"
    );
}
