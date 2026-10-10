// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the end-to-end retrieve/persona isolation
//! tests for history carriers: domain-filtered hybrid retrieve, the absence of
//! any unbounded full-log read API, and the persona channel staying unchanged
//! when failures exist.
//!
//! ## Owns
//! - retrieve `domain=failure` returns only failure-typed hits
//! - no list-all/full-log API exists
//! - `compose_context` persona section is byte-identical with failures
//!
//! ## Does not own
//! - Store-level history contract tests (`clio-store`) or the predicate-level
//!   domain checks (`domain_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::test_support::{T0, failure_args, passing_env, retriever, setup, task_args, unique};
use crate::{FailureRecordOutcome, TaskUpsertOutcome, failure_record, task_upsert};
use clio_retrieve::{
    ComposeRequest, Composer, DEFAULT_BUDGET_TOKENS, PersonaEntry, RetrieveRequest,
};
use clio_store::{IndexStore, Store};
use clio_types::{EpisodicType, EpistemicKind, HistoryPolicy, ItemKind, MemoryItem, TimeAxis};
use std::sync::Arc;

fn episodic_item(id: &str, bank: &str, episodic: EpisodicType, gist: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: "subject-1".into(),
        kind: ItemKind::Episodic,
        category: None,
        episodic_type: Some(episodic),
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        admission_score: Some(0.5),
        source_ref: None,
        snapshot: None,
        gist: Some(gist.into()),
        valid_from: T0.into(),
        valid_until: None,
        tx_from: T0.into(),
        tx_until: None,
        created_at: T0.into(),
        updated_at: T0.into(),
        context: None,
    }
}

#[test]
fn t11_retrieve_domain_failure_returns_only_failure_hits() {
    let setup = setup();
    let env = passing_env();
    let policy = HistoryPolicy::default();
    let steps = vec!["step".to_owned()];
    let TaskUpsertOutcome::Stored { record: task, .. } = task_upsert(&task_args(
        &setup,
        &env,
        "task-a",
        "login retry boom",
        &steps,
        "done",
    ))
    .unwrap() else {
        panic!("expected stored task");
    };
    let FailureRecordOutcome::Stored {
        record: failure, ..
    } = failure_record(&failure_args(
        &setup,
        &env,
        &policy,
        "task-a",
        1,
        "boom",
        "boom lesson",
    ))
    .unwrap()
    else {
        panic!("expected stored failure");
    };
    let unrelated = unique("retrieve-unrelated");
    let bank = setup.ctx.bank.as_str().to_owned();
    setup
        .store
        .create_memory_item(
            &episodic_item(&unrelated, &bank, EpisodicType::Gist, "boom unrelated"),
            "agent",
        )
        .unwrap();

    setup
        .store
        .put_lexical_doc(&bank, &task.item_id, "boom task")
        .unwrap();
    setup
        .store
        .put_lexical_doc(&bank, &failure.item_id, "boom failure")
        .unwrap();
    setup
        .store
        .put_lexical_doc(&bank, &unrelated, "boom unrelated")
        .unwrap();

    let retriever = retriever(&setup);
    let mut req = RetrieveRequest::new(&bank, "boom");
    req.domains = vec!["failure".into()];
    let out = retriever.retrieve(&req).expect("retrieve");
    let ids: Vec<String> = out.hits.iter().map(|h| h.hit.item_id.clone()).collect();
    assert_eq!(
        ids,
        vec![failure.item_id],
        "failure domain must select only the failure carrier"
    );
}

#[test]
fn t12_no_full_log_api_exists() {
    // Guard against an unbounded transcript/list-all read being added later.
    const SOURCES: &[&str] = &[
        include_str!("lib.rs"),
        include_str!("task.rs"),
        include_str!("failure.rs"),
        include_str!("retry.rs"),
        include_str!("temporal.rs"),
        include_str!("similar.rs"),
    ];
    for src in SOURCES {
        for banned in ["list_all_messages", "full_log", "dump_all", "replay_all"] {
            assert!(
                !src.contains(banned),
                "unbounded read API `{banned}` must not exist"
            );
        }
    }
    // Every read resolves to a hard cap, even when a caller asks for more.
    let policy = HistoryPolicy::default();
    assert_eq!(
        policy.resolve_limit(Some(usize::MAX)).unwrap(),
        clio_types::MAX_HISTORY_LIMIT
    );
}

#[test]
fn t13_failures_never_appear_in_the_persona_section() {
    let setup = setup();
    let env = passing_env();
    let policy = HistoryPolicy::default();
    let persona = vec![PersonaEntry {
        key: "tone".into(),
        text: "be concise".into(),
        score: 1.0,
    }];
    let bank = setup.ctx.bank.as_str().to_owned();
    let composer = Composer::new(Arc::new(retriever(&setup)));
    let request = || ComposeRequest {
        bank: bank.clone(),
        query: Some("boom".into()),
        domains: vec!["failure".into()],
        budget_tokens: DEFAULT_BUDGET_TOKENS,
        persona_entries: persona.clone(),
        as_of: None,
        time_axis: TimeAxis::Valid,
        expand_graph: false,
        now: Some(T0.into()),
        ..ComposeRequest::new(bank.clone())
    };

    let before = composer.compose(&request(), None).expect("compose");
    let persona_before = before.sections[0].text.clone();

    let FailureRecordOutcome::Stored {
        record: failure, ..
    } = failure_record(&failure_args(
        &setup,
        &env,
        &policy,
        "task-a",
        1,
        "boom",
        "re-read the fixture before patching",
    ))
    .unwrap()
    else {
        panic!("expected stored failure");
    };
    setup
        .store
        .put_lexical_doc(&bank, &failure.item_id, "boom failure")
        .unwrap();

    let after = composer.compose(&request(), None).expect("compose");
    assert_eq!(after.sections[0].name, "persona");
    assert_eq!(
        after.sections[0].text, persona_before,
        "persona section must be byte-identical with a failure present"
    );
    assert!(!after.sections[0].text.contains("fixture"));
    assert!(!after.sections[0].text.contains("re-read"));
    // The failure is still reachable through the memory section.
    assert!(
        after.sections[1].item_ids.contains(&failure.item_id),
        "failure belongs in the memory section, not persona"
    );
}
