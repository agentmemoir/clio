// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0

use crate::extract::{
    ExtractCandidate, ExtractRequest, Extractor, FixtureExtractor, PoisonExtractor,
};
use crate::pipeline::{ExtractStoreArgs, extract_verify_store};
use crate::schema::default_event_fields;
use crate::store_path::VerifiedStoreRequest;
use crate::telemetry::VerifyVecSink;
use crate::verify::VerifyConfig;
use clio_admission::decision::Candidate;
use clio_admission::gated::SignalSource;
use clio_admission::{AdmissionPolicy, RankingEnv, VecSink};
use clio_types::{
    Actor, AmError, Bank, EpistemicKind, ItemKind, MemoryItem, OpContext, SemanticCategory,
};
use serde_json::json;
use std::sync::Mutex;

#[derive(Debug, Default)]
struct EmptyNeighbors;

impl SignalSource for EmptyNeighbors {
    fn neighbor_texts(&self, _item: &MemoryItem) -> Result<Vec<String>, AmError> {
        Ok(vec![])
    }
}

#[derive(Debug)]
struct RetryThenGood {
    calls: Mutex<u32>,
}

impl Extractor for RetryThenGood {
    fn extract(&self, req: &ExtractRequest) -> Result<ExtractCandidate, AmError> {
        let mut n = self.calls.lock().unwrap();
        *n += 1;
        if *n == 1 {
            return Ok(ExtractCandidate {
                snapshot: Some(json!({"entity": "WRONG"})),
                gist: Some("bad".into()),
                source_ref: "t".into(),
                diagnostic: None,
            });
        }
        assert!(req.verify_feedback.is_some());
        Ok(ExtractCandidate {
            snapshot: Some(json!({"entity": "Ada"})),
            gist: Some("good".into()),
            source_ref: "t".into(),
            diagnostic: None,
        })
    }
}

fn base_item() -> MemoryItem {
    MemoryItem {
        id: "p1".into(),
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

fn parts<'a>(
    ctx: &'a OpContext,
    env: &'a RankingEnv,
    admit: &'a VecSink,
    verify_sink: &'a VerifyVecSink,
    fields: &'a [crate::schema::FieldSpec],
) -> VerifiedStoreRequest<'a> {
    VerifiedStoreRequest {
        item: base_item(),
        ctx,
        source_text: None,
        evidence_ref: None,
        fields,
        verify: VerifyConfig::default(),
        policy: AdmissionPolicy::new(env),
        admit_sink: admit,
        verify_sink,
        timestamp: "t0",
    }
}

#[test]
fn t05_retry_once_then_admit() {
    let ctx = OpContext::new(Bank::new("bank-1"), Actor::Agent);
    let env = RankingEnv::system_defaults();
    let admit = VecSink::default();
    let verify_sink = VerifyVecSink::default();
    let fields = default_event_fields();
    let mut store = parts(&ctx, &env, &admit, &verify_sink, &fields);
    let extractor = RetryThenGood {
        calls: Mutex::new(0),
    };
    let mut wrote = 0u32;
    let out = extract_verify_store(
        ExtractStoreArgs {
            extractor: &extractor,
            source_text: "Ada was here",
            schema_template: json!({"entity": ""}),
            tool_payload: None,
            fields: &fields,
            verify: &VerifyConfig::default(),
            item_base: base_item(),
            candidate: Candidate::Semantic(SemanticCategory::Persona),
            signal_source: &EmptyNeighbors,
        },
        &mut store,
        &mut |_| {
            wrote += 1;
            Ok(())
        },
    )
    .expect("pipeline");
    assert!(out.decision.pass, "{:?}", out.decision);
    assert_eq!(out.extract_calls, 2);
    assert_eq!(wrote, 1);
    assert!(out.retained.is_none());
}

#[test]
fn t06_fail_twice_refuses_snapshot_keeps_diagnostic() {
    let ctx = OpContext::new(Bank::new("bank-1"), Actor::Agent);
    let env = RankingEnv::system_defaults();
    let admit = VecSink::default();
    let verify_sink = VerifyVecSink::default();
    let fields = default_event_fields();
    let mut store = parts(&ctx, &env, &admit, &verify_sink, &fields);
    let poison = PoisonExtractor::new("poison_turn");
    let mut wrote = false;
    let out = extract_verify_store(
        ExtractStoreArgs {
            extractor: &poison,
            source_text: "Ada was here",
            schema_template: json!({}),
            tool_payload: None,
            fields: &fields,
            verify: &VerifyConfig::default(),
            item_base: base_item(),
            candidate: Candidate::Semantic(SemanticCategory::Persona),
            signal_source: &EmptyNeighbors,
        },
        &mut store,
        &mut |_| {
            wrote = true;
            Ok(())
        },
    )
    .expect("refuse");
    assert!(!out.decision.pass);
    assert_eq!(out.extract_calls, 2);
    assert!(!wrote);
    assert_eq!(poison.call_count(), 2);
    let retained = out.retained.expect("diagnostic retention");
    assert!(retained.gist.is_some());
    assert!(retained.diagnostic.is_some());
    let events = verify_sink.events();
    assert!(events.iter().any(|e| e.operation == "verify_refuse"));
}

#[test]
fn extractor_with_no_snapshot_retries_then_refuses() {
    #[derive(Debug)]
    struct EmptySnap;
    impl Extractor for EmptySnap {
        fn extract(&self, _req: &ExtractRequest) -> Result<ExtractCandidate, AmError> {
            Ok(ExtractCandidate {
                snapshot: None,
                gist: Some("only gist".into()),
                source_ref: "e".into(),
                diagnostic: None,
            })
        }
    }
    let ctx = OpContext::new(Bank::new("bank-1"), Actor::Agent);
    let env = RankingEnv::system_defaults();
    let admit = VecSink::default();
    let verify_sink = VerifyVecSink::default();
    let fields = default_event_fields();
    let mut store = parts(&ctx, &env, &admit, &verify_sink, &fields);
    let out = extract_verify_store(
        ExtractStoreArgs {
            extractor: &EmptySnap,
            source_text: "Ada",
            schema_template: json!({}),
            tool_payload: Some(json!({"k": 1})),
            fields: &fields,
            verify: &VerifyConfig::default(),
            item_base: base_item(),
            candidate: Candidate::Semantic(SemanticCategory::Persona),
            signal_source: &EmptyNeighbors,
        },
        &mut store,
        &mut |_| Ok(()),
    )
    .expect("refuse");
    assert!(!out.decision.pass);
    assert_eq!(out.extract_calls, 2);
    assert!(out.retained.is_some());
}

#[test]
fn t11_secret_bearing_fail_path_redacts_logs() {
    let mut fx = FixtureExtractor::new();
    fx.insert(
        "Ada api_key: sk-SECRETKEY99",
        ExtractCandidate {
            snapshot: Some(json!({"entity": "WRONG", "api_key": "sk-SECRETKEY99"})),
            gist: Some("api_key: sk-SECRETKEY99".into()),
            source_ref: "sec".into(),
            diagnostic: Some(json!({"token": "abcdEFGH9999"})),
        },
    );
    let ctx = OpContext::new(Bank::new("bank-1"), Actor::Agent);
    let env = RankingEnv::system_defaults();
    let admit = VecSink::default();
    let verify_sink = VerifyVecSink::default();
    let fields = default_event_fields();
    let mut store = parts(&ctx, &env, &admit, &verify_sink, &fields);
    let _ = extract_verify_store(
        ExtractStoreArgs {
            extractor: &fx,
            source_text: "Ada api_key: sk-SECRETKEY99",
            schema_template: json!({}),
            tool_payload: None,
            fields: &fields,
            verify: &VerifyConfig::default(),
            item_base: base_item(),
            candidate: Candidate::Semantic(SemanticCategory::Persona),
            signal_source: &EmptyNeighbors,
        },
        &mut store,
        &mut |_| Ok(()),
    )
    .expect("refuse path");
    let blob = serde_json::to_string(&verify_sink.events()).unwrap();
    assert!(!blob.contains("sk-SECRETKEY99"));
    assert!(!blob.contains("abcdEFGH9999"));
}
