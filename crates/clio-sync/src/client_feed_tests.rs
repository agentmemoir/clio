// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the outgoing change-feed packaging: closed triples are
//! packaged as `invalidate` mutations and replicate end to end, discarded
//! items are packaged as `discard` mutations, and persona/belief domain
//! records replicate end to end (push → relay → pull → local domain rows).
//!
//! ## Owns
//! - Behavior coverage for `client_feed` (record, triple, and discard packaging)
//!
//! ## Does not own
//! - Item/assoc packaging (`client_tests` / `client_status_tests`)
//! - Apply semantics (`apply_tests`)
//!
//! ## Boundary
//!
//! Tests here break when push-side packaging changes; they never change it.
//!
//! Keep this module focused on change-feed packaging tests.

use crate::client::client_tests::{cfg, paired_cfg, serve, sqlite};
use crate::client::{pull, push};
use clio_store::{BeliefStore, PersonaStore, Store, SyncStore};
use clio_types::{
    BeliefObject, ConfidenceEntry, EpisodicType, EpistemicKind, ItemKind, MemoryItem,
    PersonaPreference, PersonaStableEntry, SourceType, TripleRecord,
};
use serde_json::json;

fn carrier(id: &str, bank: &str, object: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: "subj".into(),
        kind: ItemKind::Episodic,
        category: None,
        episodic_type: Some(EpisodicType::Triple),
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.9),
        admission_score: Some(0.85),
        source_ref: Some("turn".into()),
        snapshot: Some(json!({"object": object})),
        gist: Some(format!("lives_in {object}")),
        valid_from: "2026-01-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2026-01-01T00:00:00Z".into(),
        tx_until: None,
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-01-01T00:00:00Z".into(),
        context: None,
    }
}

fn triple_edge(id: &str, bank: &str, item_id: &str) -> TripleRecord {
    TripleRecord {
        context: None,
        id: id.into(),
        bank_id: bank.into(),
        item_id: item_id.into(),
        subject: "Alice".into(),
        predicate: "lives_in".into(),
        object: "Boston".into(),
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.9),
        valid_from: "2026-01-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2026-01-01T00:00:00Z".into(),
        tx_until: None,
        snapshot_ref: None,
        gist_ref: None,
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: None,
    }
}

#[test]
fn triple_close_packages_as_invalidate_and_replicates() {
    let (server, srv, _kms) = serve(&["bt"]);
    let addr = server.addr().to_owned();
    let a = sqlite();
    let b = sqlite();
    let cfg_a = paired_cfg("dev-a", &a, &addr);
    let cfg_b = paired_cfg("dev-b", &b, &addr);
    // A owns the triple; B independently holds the same SPO identity.
    a.commit_triple_add(
        &carrier("car-a", "bt", "Boston"),
        &triple_edge("e-a", "bt", "car-a"),
        true,
        "agent",
    )
    .expect("a triple");
    b.commit_triple_add(
        &carrier("car-b", "bt", "Boston"),
        &triple_edge("e-b", "bt", "car-b"),
        true,
        "agent",
    )
    .expect("b triple");
    // A closes the triple: the change feed must emit a `triple` invalidate.
    let ended = a
        .triple_end(
            "bt",
            "Alice",
            "lives_in",
            Some("Boston"),
            "2026-02-01T00:00:00Z",
            "2026-02-01T00:00:00Z",
        )
        .expect("end triple");
    assert_eq!(ended.closed_ids, vec!["e-a".to_owned()]);
    push(&cfg_a, &["bt".to_owned()]).expect("push close");
    let relayed = srv.journal_list_since("bt", 0, 50).expect("server journal");
    assert!(
        relayed.iter().any(|r| r.entity_kind == "triple"),
        "relay must journal the triple invalidate: {relayed:?}"
    );
    pull(&cfg_b, &["bt".to_owned()]).expect("pull close");
    let closed = b.get_triple("bt", "e-b").unwrap().expect("b triple");
    assert_eq!(
        closed.valid_until.as_deref(),
        Some("2026-02-01T00:00:00Z"),
        "peer triple closes under the synced invalidate"
    );
    server.stop();
}

/// Exact restored values from the replicated payload (bit-identical round trip).
#[allow(clippy::float_cmp)]
#[test]
fn persona_and_belief_records_replicate_end_to_end() {
    let (server, srv, _kms) = serve(&["br"]);
    let addr = server.addr().to_owned();
    let a = sqlite();
    let b = sqlite();
    let cfg_a = paired_cfg("dev-a", &a, &addr);
    let cfg_b = paired_cfg("dev-b", &b, &addr);
    // Seed the domain records on host A (persona stable + preference, belief).
    let entry = PersonaStableEntry {
        bank_id: "br".into(),
        key: "style".into(),
        value: "terse".into(),
        confidence: None,
        source: "explicit".into(),
        source_ref: Some("turn-1".into()),
        context: Some("why recorded".into()),
        admission_score: 0.5,
        as_of: "2026-01-01".into(),
        valid_until: None,
    };
    a.put_stable_entry(&entry, "seed").expect("persona stable");
    let pref = PersonaPreference {
        bank_id: "br".into(),
        key: "brevity".into(),
        value: 0.4,
        trend: 0.1,
        confidence: None,
        source: "inferred".into(),
        source_ref: None,
        context: Some("preference context".into()),
        admission_score: 0.5,
        as_of: "2026-01-01".into(),
    };
    a.put_preference_state(&pref, "seed").expect("preference");
    let mut belief = BeliefObject {
        id: "bel-1".into(),
        bank_id: "br".into(),
        subject_id: "s1".into(),
        proposition: "user prefers oat milk".into(),
        context: Some("stated twice".into()),
        context_as_of: None,
        confidence_history: vec![
            ConfidenceEntry::new("2026-01-01", 0.7, "ev-1", SourceType::UserStated).expect("entry"),
        ],
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-01-01T00:00:00Z".into(),
    };
    a.create_belief(&belief, "seed").expect("belief");
    push(&cfg_a, &["br".to_owned()]).expect("push records");
    let relayed = srv.journal_list_since("br", 0, 50).expect("server journal");
    assert!(
        relayed.iter().any(|r| r.entity_kind == "persona"),
        "persona records must reach the relay journal: {relayed:?}"
    );
    assert!(
        relayed.iter().any(|r| r.entity_kind == "belief"),
        "belief records must reach the relay journal: {relayed:?}"
    );
    // Host B pulls and materializes the same domain records.
    pull(&cfg_b, &["br".to_owned()]).expect("pull records");
    let doc = b.get_persona_document("br").expect("b persona doc");
    assert_eq!(doc.stable.len(), 1);
    assert_eq!(doc.stable[0].value, "terse");
    assert_eq!(doc.stable[0].context.as_deref(), Some("why recorded"));
    let b_pref = b.get_preference("br", "brevity").unwrap().expect("b pref");
    assert_eq!(b_pref.value, 0.4);
    assert_eq!(b_pref.context.as_deref(), Some("preference context"));
    let b_belief = b.get_belief("br", "bel-1").unwrap().expect("b belief");
    assert_eq!(b_belief.proposition, "user prefers oat milk");
    assert_eq!(b_belief.context.as_deref(), Some("stated twice"));
    assert_eq!(b_belief.confidence_history.len(), 1);
    // A later append replicates as the missing trajectory tail.
    belief
        .append(
            ConfidenceEntry::new("2026-01-02", 0.9, "ev-2", SourceType::UserStated).expect("entry"),
        )
        .expect("append");
    a.append_confidence("br", "bel-1", &belief.confidence_history[1], "seed")
        .expect("a append");
    push(&cfg_a, &["br".to_owned()]).expect("push append");
    pull(&cfg_b, &["br".to_owned()]).expect("pull append");
    let b_belief = b.get_belief("br", "bel-1").unwrap().expect("b belief");
    assert_eq!(b_belief.confidence_history.len(), 2);
    // Idempotent re-push adds nothing new to the relay journal.
    let before = srv.journal_max_seq("br").unwrap();
    push(&cfg_a, &["br".to_owned()]).expect("re-push");
    assert_eq!(srv.journal_max_seq("br").unwrap(), before);
    server.stop();
}

/// A task and a failure record (carrier item + structured row, both with
/// context) pushed from node A must materialize on node B so `task_get` and
/// `list_failures` return them with their context after pull; a redelivery must
/// stay idempotent (no duplicate rows, no version/attempt bump).
#[test]
fn task_and_failure_records_replicate_end_to_end() {
    use clio_store::HistoryStore;
    use clio_types::{FailureRecord, TaskRecord};

    let (server, srv, _kms) = serve(&["btf"]);
    let addr = server.addr().to_owned();
    let a = sqlite();
    let b = sqlite();
    let cfg_a = paired_cfg("dev-a", &a, &addr);
    let cfg_b = paired_cfg("dev-b", &b, &addr);

    let mut task_item = carrier("car-task", "btf", "obj");
    task_item.episodic_type = Some(EpisodicType::Task);
    task_item.gist = Some("do the thing".into());
    task_item.context = Some("task why".into());
    let task = TaskRecord {
        id: "trec-1".into(),
        bank_id: "btf".into(),
        item_id: "car-task".into(),
        task_id: "task-1".into(),
        subject_id: "subj".into(),
        version: 1,
        definition: "do the thing".into(),
        steps: vec!["step-a".into()],
        status: "open".into(),
        evidence_ref: Some("turn".into()),
        context: None,
        admission_score: 0.85,
        created_at: "2026-01-01T00:00:00Z".into(),
    };
    a.commit_task_upsert(&task_item, &task, "agent")
        .expect("a task");

    let mut failure_item = carrier("car-fail", "btf", "obj");
    failure_item.episodic_type = Some(EpisodicType::Failure);
    failure_item.epistemic_kind = EpistemicKind::Belief;
    failure_item.source_type = Some(SourceType::AgentInferred);
    failure_item.gist = Some("what failed".into());
    failure_item.context = Some("failure why".into());
    let failure = FailureRecord {
        id: "frec-1".into(),
        bank_id: "btf".into(),
        item_id: "car-fail".into(),
        task_id: "task-1".into(),
        subject_id: "subj".into(),
        attempt_n: 1,
        what_failed: "boom".into(),
        lesson: "don't".into(),
        lesson_tokens: 1,
        evidence_ref: None,
        context: None,
        admission_score: 0.85,
        epistemic_kind: EpistemicKind::Belief,
        created_at: "2026-01-01T00:00:00Z".into(),
    };
    a.commit_failure_record(&failure_item, &failure, "agent")
        .expect("a failure");

    push(&cfg_a, &["btf".to_owned()]).expect("push");
    let relayed = srv
        .journal_list_since("btf", 0, 50)
        .expect("server journal");
    assert!(
        relayed.iter().any(|r| r.entity_kind == "task"),
        "task records must reach the relay journal: {relayed:?}"
    );
    assert!(
        relayed.iter().any(|r| r.entity_kind == "failure"),
        "failure records must reach the relay journal: {relayed:?}"
    );
    pull(&cfg_b, &["btf".to_owned()]).expect("pull");

    let got_task = b
        .get_task("btf", "task-1")
        .expect("task get")
        .expect("task replicated");
    assert_eq!(got_task.definition, "do the thing");
    assert_eq!(got_task.version, 1);
    assert_eq!(got_task.context.as_deref(), Some("task why"));
    let got_failure = b.list_failures("btf", 10).expect("list failures");
    assert_eq!(got_failure.len(), 1);
    assert_eq!(got_failure[0].what_failed, "boom");
    assert_eq!(got_failure[0].context.as_deref(), Some("failure why"));

    // Redelivery is idempotent: no new rows, no version/attempt bump.
    let seq = srv.journal_max_seq("btf").unwrap();
    push(&cfg_a, &["btf".to_owned()]).expect("re-push");
    assert_eq!(srv.journal_max_seq("btf").unwrap(), seq);
    pull(&cfg_b, &["btf".to_owned()]).expect("re-pull");
    assert_eq!(
        b.task_history("btf", "task-1", 10).expect("history").len(),
        1
    );
    assert_eq!(b.list_failures("btf", 10).expect("failures").len(), 1);
    assert_eq!(
        b.get_task("btf", "task-1")
            .expect("get")
            .expect("task")
            .version,
        1
    );
    server.stop();
}

#[test]
fn triple_packaging_skips_open_and_missing_triples() {
    let a = sqlite();
    let cfg_a = cfg("dev-a", &a, "127.0.0.1:1", Vec::new());
    a.commit_triple_add(
        &carrier("car-a", "bt", "Boston"),
        &triple_edge("e-a", "bt", "car-a"),
        true,
        "agent",
    )
    .expect("open triple");
    assert!(
        crate::client_feed::package::triple_mutation(&cfg_a, "bt", "e-a")
            .expect("package open")
            .is_none()
    );
    assert!(
        crate::client_feed::package::triple_mutation(&cfg_a, "bt", "e-missing")
            .expect("package missing")
            .is_none()
    );
}

#[test]
fn discard_feed_packages_and_replicates_discard_op() {
    let (server, srv, _kms) = serve(&["bd"]);
    let addr = server.addr().to_owned();
    let a = sqlite();
    let cfg_a = paired_cfg("dev-a", &a, &addr);
    a.create_memory_item(
        &crate::client::client_tests::item("bd", "i-1", "s1", "2026-01-01T00:00:00Z", "g"),
        "actor",
    )
    .expect("create");
    push(&cfg_a, &["bd".to_owned()]).expect("push create");
    a.discard_item("bd", "i-1", "superseded", "2026-01-02T00:00:00Z", "actor")
        .expect("discard");
    push(&cfg_a, &["bd".to_owned()]).expect("push discard");
    let rows = srv.journal_list_since("bd", 0, 50).expect("server journal");
    assert!(
        rows.iter().any(|r| r.op == "discard"),
        "discard op must reach the relay journal: {rows:?}"
    );
    server.stop();
}
