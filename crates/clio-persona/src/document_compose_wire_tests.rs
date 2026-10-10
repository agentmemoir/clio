// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the compose-channel integration test: the
//! persona document assembled by `persona_get` feeds `clio-retrieve`'s Composer
//! and survives the intent-gate skip path.
//!
//! ## Owns
//! - compose after intent skip keeps a non-empty persona section
//! - persona section tokens never exceed the persona budget
//!
//! ## Boundary
//!
//! Compose packing internals belong to `clio-retrieve`; tests here only prove the
//! wiring contract (entry shape + budget agreement).

use crate::budget::persona_pack_entries;
use crate::document::{PersonaGetArgs, persona_get};
use crate::stable::PersonaPutStableArgs;
use crate::stable::stable_tests::{EmptySignals, Setup, setup};
use crate::{DEFAULT_PERSONA_TOKEN_BUDGET, PersonaGetOutcome, persona_put_stable};
use clio_admission::{AdmissionPolicy, RankingEnv};
use clio_retrieve::{ComposeRequest, Composer, GateDecision, HybridRetriever, PersonaEntry};
use clio_store::{IndexStore, Store};
use std::sync::Arc;

fn compose_entries(outcome: &PersonaGetOutcome) -> Vec<PersonaEntry> {
    persona_pack_entries(&outcome.document)
        .into_iter()
        .map(|e| PersonaEntry {
            key: e.key,
            text: e.text,
            score: e.score,
        })
        .collect()
}

fn put<'a>(s: &'a Setup, key: &'a str, value: &'a str) {
    let env = RankingEnv::default();
    let args = PersonaPutStableArgs {
        context: None,
        store: &s.store,
        continuous: &s.store,
        ctx: &s.ctx,
        key,
        value,
        confidence: None,
        source: Some("explicit"),
        evidence_ref: None,
        policy: AdmissionPolicy::new(&env),
        signal_source: &EmptySignals,
        admission_sink: &s.admit_sink,
        persona_sink: &s.persona_sink,
        budget_tokens: DEFAULT_PERSONA_TOKEN_BUDGET,
        timestamp: "2026-09-10",
        dry_run: false,
    };
    persona_put_stable(&args).unwrap();
}

/// With the gate skipping retrieval, the persona section is still
/// non-empty and within the persona budget; memory stays empty.
#[test]
fn compose_after_intent_skip_includes_persona_section() {
    let s = setup();
    put(&s, "name", "Ada");
    put(&s, "communication_style", "concise");
    let get = persona_get(&PersonaGetArgs {
        store: &s.store,
        ctx: &s.ctx,
        budget_tokens: DEFAULT_PERSONA_TOKEN_BUDGET,
        timestamp: "2026-09-10",
    })
    .unwrap();
    assert!(!get.document.stable.is_empty(), "{:?}", get.document.stable);
    let composer = retriever_for(&s);
    let pack = composer
        .compose(&request(&s, compose_entries(&get)), Some(&skip_gate()))
        .unwrap();
    let persona = pack
        .sections
        .iter()
        .find(|sec| sec.name == "persona")
        .expect("persona section");
    assert!(!persona.text.is_empty(), "persona section must be filled");
    assert!(persona.tokens <= DEFAULT_PERSONA_TOKEN_BUDGET);
    let memory = pack
        .sections
        .iter()
        .find(|sec| sec.name == "memory")
        .expect("memory section");
    assert!(memory.text.is_empty(), "{:?}", memory.text);
    assert!(!pack.retrieval_ran);
}

/// Compose pack for an oversized document stays inside the persona budget and
/// drops exactly what `persona_get` drops (single source of truth).
#[test]
fn compose_and_persona_get_see_the_same_truncated_document() {
    let s = setup();
    for i in 0..20 {
        put(
            &s,
            &format!("key-{i}"),
            &"value-value-value-value-".repeat(5),
        );
    }
    let get = persona_get(&PersonaGetArgs {
        store: &s.store,
        ctx: &s.ctx,
        budget_tokens: DEFAULT_PERSONA_TOKEN_BUDGET,
        timestamp: "t",
    })
    .unwrap();
    assert!(!get.dropped_keys.is_empty(), "fixture must overflow budget");
    let composer = retriever_for(&s);
    let pack = composer
        .compose(&request(&s, compose_entries(&get)), None)
        .unwrap();
    let persona = pack
        .sections
        .iter()
        .find(|sec| sec.name == "persona")
        .expect("persona section");
    let included: Vec<String> = persona.item_ids.clone();
    let visible: Vec<String> = get.document.stable.iter().map(|e| e.key.clone()).collect();
    assert_eq!(
        included, visible,
        "compose sees exactly the persona_get view"
    );
    assert!(persona.tokens <= DEFAULT_PERSONA_TOKEN_BUDGET);
}

fn retriever_for(_s: &Setup) -> Composer {
    let kms: Arc<dyn clio_store::DekProvider> = Arc::new(clio_store::LocalDevKms::new());
    let store =
        Arc::new(clio_store::SqliteStore::open_in_memory(kms).expect("sqlite")) as Arc<dyn Store>;
    let index = Arc::new(
        clio_store::SqliteStore::open_in_memory(Arc::new(clio_store::LocalDevKms::new()))
            .expect("sqlite"),
    ) as Arc<dyn IndexStore>;
    Composer::new(Arc::new(HybridRetriever::new(store, index)))
}

fn skip_gate() -> GateDecision {
    GateDecision {
        retrieval_needed: false,
        domains: Vec::new(),
        confidence: 0.95,
        reason: "test skip".into(),
    }
}

fn request(s: &Setup, persona_entries: Vec<PersonaEntry>) -> ComposeRequest {
    ComposeRequest {
        bank: s.ctx.bank.as_str().to_owned(),
        query: None,
        domains: vec![],
        budget_tokens: 2048,
        persona_entries,
        as_of: None,
        time_axis: clio_types::TimeAxis::Valid,
        expand_graph: false,
        now: None,
        ..ComposeRequest::new(s.ctx.bank.as_str())
    }
}
