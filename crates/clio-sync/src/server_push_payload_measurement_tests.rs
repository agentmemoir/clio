// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module measures the largest request payload the sync client feed
//! actually produces against the transport's declared body cap: one full feed
//! page per packaged kind (8 kinds, `PULL_PAGE_LIMIT` rows each), seeded
//! through real store writes, packaged and serialized exactly as `push_bank`
//! does (`to_value` then `to_vec`).
//!
//! ## Owns
//! - The page-full push payload size measurement against `BODY_CAP_BYTES`
//!
//! ## Does not own
//! - The bound constants and their in-loop enforcement (`http`)
//! - Route semantics and the other transport-bound behavior tests
//!   (`server_conn_tests`)
//!
//! ## Boundary
//!
//! Tests here break when the feed packaging or the body cap changes; they
//! never change it. Content is realistic-but-large (a few KiB per
//! snapshot/gist). Caveat: item `snapshot` is caller-provided JSON with no
//! byte cap at the tool boundary and `context` is capped at 4 KiB
//! (`CONTEXT_MAX_BYTES`), so this covers realistic content, not adversarial
//! snapshots.
//!
//! Keep this module focused on the push payload size measurement.

use super::server_tests::sqlite;

#[allow(clippy::too_many_lines)]
#[test]
fn page_full_push_payload_fits_inside_the_body_cap() {
    use crate::client::SyncClientConfig;
    use crate::client::client_tests::item as base_item;
    use crate::client_feed::kind_watermark;
    use crate::client_feed::package::{
        assoc_mutation, belief_mutation, failure_mutation, item_mutation, persona_pref_mutation,
        persona_stable_mutation, task_mutation, triple_mutation,
    };
    use crate::http::BODY_CAP_BYTES;
    use crate::protocol::SyncPayload;
    use clio_store::{AssocEdgeRecord, BeliefStore, HistoryStore, PersonaStore, Store, SyncStore};
    use clio_types::{
        BeliefObject, EpisodicType, EpistemicKind, FailureRecord, ItemKind, MemoryItem,
        PersonaPreference, PersonaStableEntry, SourceType, TaskRecord, TripleRecord,
    };
    use serde_json::json;

    // Dense JSON fixtures (missing Option fields deserialize as None; macro
    // bodies stay readable and rustfmt-stable).
    fn fixture<T: serde::de::DeserializeOwned>(v: serde_json::Value) -> T {
        serde_json::from_value(v).expect("valid fixture record")
    }

    let bank = "bmeas";
    let n = crate::PULL_PAGE_LIMIT;
    let (store, _kms) = sqlite();
    // Big items sit at hour 01 so the item feed page is entirely the large
    // items; carrier items sit at later hours. Shared shared strings below.
    let stamp_at =
        |hour: u32, i: usize| format!("2026-01-01T{hour:02}:{:02}:{:02}Z", i / 60, i % 60);
    let big_item = |id: &str, hour: u32, i: usize| MemoryItem {
        snapshot: Some(json!({ "payload": "s".repeat(2048) })),
        gist: Some("g".repeat(1024)),
        ..base_item(bank, id, "s1", &stamp_at(hour, i), "gist")
    };
    let ctx = "c".repeat(1024);
    let steps = vec!["s".repeat(512); 4];
    let close = "2026-02-01T00:00:00Z";

    for i in 0..n {
        let id = format!("{i:03}");
        store
            .create_memory_item(&big_item(&format!("i-{id}"), 1, i), "actor")
            .expect("create item");
        let edge: AssocEdgeRecord = fixture(json!({
            "id": format!("e-{id}"), "bank_id": bank,
            "source_id": format!("i-{id}"), "target_id": format!("i-{:03}", (i + 1) % n),
            "assoc_kind": "coactivation", "weight": 0.5, "last_updated_at": stamp_at(2, i),
        }));
        store.put_assoc_edge(&edge).expect("put edge");
        let obj = format!("obj-{id}");
        let triple_carrier = MemoryItem {
            kind: ItemKind::Episodic,
            category: None,
            episodic_type: Some(EpisodicType::Triple),
            snapshot: Some(json!({ "payload": "s".repeat(512) })),
            gist: Some("g".repeat(256)),
            ..big_item(&format!("car-t-{id}"), 7, i)
        };
        let triple: TripleRecord = fixture(json!({
            "id": format!("tri-{id}"), "bank_id": bank, "item_id": format!("car-t-{id}"),
            "subject": "Alice", "predicate": "lives_in", "object": obj,
            "epistemic_kind": "fact", "confidence": 0.9,
            "valid_from": "2026-01-01T00:00:00Z", "tx_from": "2026-01-01T00:00:00Z",
            "created_at": "2026-01-01T00:00:00Z",
        }));
        store
            .commit_triple_add(&triple_carrier, &triple, true, "agent")
            .expect("triple add");
        store
            .triple_end(bank, "Alice", "lives_in", Some(&obj), close, close)
            .expect("triple close");
        let mut stable: PersonaStableEntry = fixture(json!({
            "key": format!("k-{id}"), "value": "v".repeat(1024), "confidence": 0.5,
            "source": "explicit", "source_ref": "turn", "context": ctx.clone(),
            "admission_score": 0.5, "as_of": "2026-01-01",
        }));
        stable.bank_id = bank.into();
        store
            .put_stable_entry(&stable, "actor")
            .expect("stable entry");
        let mut pref: PersonaPreference = fixture(json!({
            "key": format!("p-{id}"), "value": 0.4, "trend": 0.1, "source": "inferred",
            "context": ctx.clone(), "admission_score": 0.5, "as_of": "2026-01-01",
        }));
        pref.bank_id = bank.into();
        store
            .put_preference_state(&pref, "actor")
            .expect("preference");
        let belief: BeliefObject = fixture(json!({
            "id": format!("bel-{id}"), "bank_id": bank, "subject_id": "s1",
            "proposition": format!("{}-{id}", "b".repeat(2048)), "context": ctx.clone(),
            "confidence_history": [{
                "as_of": "2026-01-01", "confidence": 0.7,
                "evidence_ref": "ev-1", "source_type": "user_stated",
            }],
            "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z",
        }));
        store.create_belief(&belief, "actor").expect("belief");
        let task_item = MemoryItem {
            kind: ItemKind::Episodic,
            category: None,
            episodic_type: Some(EpisodicType::Task),
            context: Some(ctx.clone()),
            snapshot: Some(json!({ "payload": "s".repeat(512) })),
            gist: Some("g".repeat(256)),
            ..big_item(&format!("car-k-{id}"), 8, i)
        };
        let task: TaskRecord = fixture(json!({
            "id": format!("trec-{id}"), "bank_id": bank, "item_id": format!("car-k-{id}"),
            "task_id": format!("task-{id}"), "subject_id": "s1", "version": 1,
            "definition": "d".repeat(1024), "steps": steps.clone(), "status": "open",
            "evidence_ref": "turn", "context": ctx.clone(),
            "admission_score": 0.8, "created_at": "2026-01-01T00:00:00Z",
        }));
        store
            .commit_task_upsert(&task_item, &task, "agent")
            .expect("task");
        let fail_item = MemoryItem {
            kind: ItemKind::Episodic,
            category: None,
            episodic_type: Some(EpisodicType::Failure),
            epistemic_kind: EpistemicKind::Belief,
            source_type: Some(SourceType::AgentInferred),
            context: Some(ctx.clone()),
            snapshot: Some(json!({ "payload": "s".repeat(512) })),
            gist: Some("g".repeat(256)),
            ..big_item(&format!("car-f-{id}"), 9, i)
        };
        let failure: FailureRecord = fixture(json!({
            "id": format!("frec-{id}"), "bank_id": bank, "item_id": format!("car-f-{id}"),
            "task_id": format!("taskf-{id}"), "subject_id": "s1", "attempt_n": 1,
            "what_failed": "w".repeat(2048), "lesson": "l".repeat(1024), "lesson_tokens": 1,
            "context": ctx.clone(), "admission_score": 0.85,
            "epistemic_kind": "belief", "created_at": "2026-01-01T00:00:00Z",
        }));
        store
            .commit_failure_record(&fail_item, &failure, "agent")
            .expect("failure");
    }

    // Package each kind's page exactly as `push_bank` packages it.
    let cfg = SyncClientConfig {
        device_id: "dev-a".into(),
        store: store.clone(),
        sync: store.clone(),
        remote: None,
        token: None,
        cipher: Vec::new(),
    };
    let page = crate::PULL_PAGE_LIMIT;
    let since = |kind: &str| kind_watermark(&cfg, bank, kind).expect("watermark");
    let mut mutations = Vec::new();
    for row in store
        .feed_items(bank, &since("item"), page)
        .expect("feed items")
    {
        let item = store
            .get_memory_item(bank, &row.entity_id)
            .expect("get")
            .expect("item");
        mutations.push(item_mutation(&cfg, &item, row.op.as_str()).expect("package item"));
    }
    for row in store
        .feed_assoc(bank, &since("assoc_edge"), page)
        .expect("feed assoc")
    {
        let edge = store
            .get_assoc_edge(bank, &row.entity_id)
            .expect("get")
            .expect("edge");
        mutations.push(assoc_mutation(&cfg, bank, &edge).expect("package edge"));
    }
    for row in store
        .feed_triples(bank, &since("triple"), page)
        .expect("feed triples")
    {
        if let Some(m) = triple_mutation(&cfg, bank, &row.entity_id).expect("package triple") {
            mutations.push(m);
        }
    }
    let stable_rows = store
        .feed_persona_stable(bank, &since("persona_stable"), page)
        .expect("feed");
    for (stamp, entry) in stable_rows {
        mutations.push(persona_stable_mutation(&cfg, bank, stamp.as_str(), &entry).expect("pkg"));
    }
    let pref_rows = store
        .feed_persona_preferences(bank, &since("persona_preference"), page)
        .expect("feed");
    for (stamp, pref) in pref_rows {
        mutations.push(persona_pref_mutation(&cfg, bank, stamp.as_str(), &pref).expect("pkg"));
    }
    let belief_rows = store
        .feed_beliefs(bank, &since("belief"), page)
        .expect("feed");
    for (stamp, belief) in belief_rows {
        mutations.push(belief_mutation(&cfg, bank, stamp.as_str(), &belief).expect("pkg"));
    }
    for (stamp, task) in store
        .feed_tasks(bank, &since("task"), page)
        .expect("feed tasks")
    {
        let item = store
            .get_memory_item(bank, &task.item_id)
            .expect("get")
            .expect("carrier");
        mutations.push(task_mutation(&cfg, bank, stamp.as_str(), &task, &item).expect("pkg"));
    }
    let failure_rows = store
        .feed_failures(bank, &since("failure"), page)
        .expect("feed");
    for (stamp, failure) in failure_rows {
        let item = store
            .get_memory_item(bank, &failure.item_id)
            .expect("get")
            .expect("carrier");
        mutations.push(failure_mutation(&cfg, bank, stamp.as_str(), &failure, &item).expect("pkg"));
    }
    assert_eq!(mutations.len(), page * 8, "each kind must fill its page");

    // Serialize exactly as `push_bank` does: `to_value` then `to_vec`.
    let payload = SyncPayload {
        device_id: "dev-a".into(),
        bank: bank.into(),
        mutations,
    };
    let wire = serde_json::to_vec(&serde_json::to_value(&payload).expect("serialize payload"))
        .expect("wire bytes");
    eprintln!("page-full push payload measured: {} bytes", wire.len());
    assert!(
        wire.len() < BODY_CAP_BYTES,
        "page-full push payload {} bytes must fit the {} MiB body cap with headroom",
        wire.len(),
        BODY_CAP_BYTES / (1024 * 1024)
    );
}
