// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for audit-row context telemetry coverage of the
//! history writes: task, failure, and triple audit rows must
//! carry PII-safe context statistics and never the raw context value.
//!
//! ## Owns
//! - Audit-context coverage for task/failure/triple writes on both backends
//!
//! ## Does not own
//! - History read/write contract coverage (`sqlite_history_tests`,
//!   `pg_history_tests`) or triple write/query semantics (`triple_tests`)
//! - Audit-trail reconstruction SQL (`sqlite_audit`, `postgres_audit`)
//!
//! ## Boundary
//!
//! Changes to audit-row context telemetry should normally update tests here.
//! Changes to the record read surface should not require changes here.
//!
//! Test-only. Never compiled into production builds.

use super::HistoryStore;
use crate::db_url::postgres_url_from_env;
use crate::dek::{DekProvider, LocalDevKms};
use crate::erase_hash::sha256_hex;
use crate::pg_lock::lock_pg_test;
use crate::postgres::PostgresStore;
use crate::sqlite::SqliteStore;
use crate::store::Store;
use clio_types::{
    EpisodicType, EpistemicKind, FailureRecord, ItemKind, MemoryItem, SourceType, TaskRecord,
    TripleRecord,
};
use serde_json::json;
use std::sync::Arc;

/// Must stay identical to the domain salt used by `item_persist::context_stats`
/// so audit-row hashes are comparable across record types.
const CTX_HASH_DOMAIN: &[u8] = b"clio-context-telemetry";

use crate::test_unique::unique;

fn sqlite_store() -> SqliteStore {
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    SqliteStore::open_in_memory(kms).expect("sqlite")
}

fn carrier(id: &str, bank: &str, episodic: EpisodicType, context: Option<&str>) -> MemoryItem {
    let belief = episodic == EpisodicType::Failure;
    MemoryItem {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: "subject-1".into(),
        kind: ItemKind::Episodic,
        category: None,
        episodic_type: Some(episodic),
        epistemic_kind: if belief {
            EpistemicKind::Belief
        } else {
            EpistemicKind::Fact
        },
        source_type: belief.then_some(SourceType::AgentInferred),
        confidence: None,
        admission_score: Some(0.7),
        source_ref: None,
        snapshot: Some(json!({"key": "value"})),
        gist: Some("carrier gist".into()),
        valid_from: "2026-06-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2026-06-01T00:00:00Z".into(),
        tx_until: None,
        created_at: "2026-06-01T00:00:00Z".into(),
        updated_at: "2026-06-01T00:00:00Z".into(),
        context: context.map(str::to_owned),
    }
}

fn task_draft(bank: &str, item_id: &str, task_id: &str, context: Option<&str>) -> TaskRecord {
    TaskRecord {
        id: unique("task"),
        bank_id: bank.into(),
        item_id: item_id.into(),
        task_id: task_id.into(),
        subject_id: "subject-1".into(),
        version: 1,
        definition: "fix the flaky login test".into(),
        steps: vec!["run tests".into()],
        status: "in_progress".into(),
        evidence_ref: None,
        context: context.map(str::to_owned),
        admission_score: 0.7,
        created_at: "2026-06-01T00:00:00Z".into(),
    }
}

fn failure_draft(
    bank: &str,
    item_id: &str,
    task_id: &str,
    attempt_n: i64,
    context: Option<&str>,
) -> FailureRecord {
    FailureRecord {
        id: unique("failure"),
        bank_id: bank.into(),
        item_id: item_id.into(),
        task_id: task_id.into(),
        subject_id: "subject-1".into(),
        attempt_n,
        what_failed: "assertion error".into(),
        lesson: "read the fixture".into(),
        lesson_tokens: 3,
        evidence_ref: None,
        context: context.map(str::to_owned),
        admission_score: 0.7,
        epistemic_kind: EpistemicKind::Belief,
        created_at: "2026-06-01T00:00:00Z".into(),
    }
}

fn edge(id: &str, bank: &str, item_id: &str, object: &str) -> TripleRecord {
    TripleRecord {
        id: id.into(),
        bank_id: bank.into(),
        item_id: item_id.into(),
        subject: "Alice".into(),
        predicate: "lives_in".into(),
        object: object.into(),
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        valid_from: "2026-06-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2026-06-01T00:00:00Z".into(),
        tx_until: None,
        snapshot_ref: None,
        gist_ref: None,
        context: None,
        created_at: "2026-06-01T00:00:00Z".into(),
        updated_at: None,
    }
}

/// The `operation` audit event's detail for one carrier item, read back
/// through the `audit_trail` read side.
fn detail_of(store: &dyn Store, bank: &str, item_id: &str, operation: &str) -> serde_json::Value {
    store
        .audit_trail(bank, item_id)
        .expect("audit trail")
        .events
        .into_iter()
        .find(|event| event.operation == operation)
        .unwrap_or_else(|| panic!("no {operation} event for {item_id}"))
        .detail
}

/// Context change between task versions: each version's audit row carries the
/// presence/length/hash of the context written by that version, the two hashes
/// differ (change detection), the raw values never appear in telemetry, and
/// the prior version's sealed context stays reconstructable via `task_history`.
#[test]
fn sqlite_task_audit_rows_carry_context_stats() {
    let store = sqlite_store();
    let bank = unique("sqlite-audit-task");
    let v1_item = unique("i-task-v1");
    let v2_item = unique("i-task-v2");
    store
        .commit_task_upsert(
            &carrier(&v1_item, &bank, EpisodicType::Task, Some("team chat")),
            &task_draft(&bank, &v1_item, "audit-task", Some("team chat")),
            "actor",
        )
        .expect("task v1");
    store
        .commit_task_upsert(
            &carrier(&v2_item, &bank, EpisodicType::Task, Some("standup notes")),
            &task_draft(&bank, &v2_item, "audit-task", Some("standup notes")),
            "actor",
        )
        .expect("task v2");

    let v1 = detail_of(&store, &bank, &v1_item, "task_upsert");
    let v2 = detail_of(&store, &bank, &v2_item, "task_upsert");
    assert_eq!(v1["record"], json!("task"));
    assert_eq!(v1["context_present"], json!(true));
    assert_eq!(v1["context_len"], json!("team chat".len()));
    assert_eq!(
        v1["context_hash"],
        json!(sha256_hex(CTX_HASH_DOMAIN, "team chat"))
    );
    assert_eq!(v2["version"], json!(2));
    assert_eq!(
        v2["context_hash"],
        json!(sha256_hex(CTX_HASH_DOMAIN, "standup notes"))
    );
    assert_ne!(v1["context_hash"], v2["context_hash"], "context changed");
    // Ordinary telemetry must not emit raw context the event list is
    // the diagnostics surface (the revisions' context is the authorized read).
    let telemetry =
        serde_json::to_string(&store.audit_trail(&bank, &v1_item).expect("trail").events).unwrap();
    assert!(!telemetry.contains("team chat"), "{telemetry}");
    // Prior context stays reconstructable through the version chain.
    let history = store
        .task_history(&bank, "audit-task", 10)
        .expect("history");
    assert_eq!(history.len(), 2);
    assert_eq!(history[1].context.as_deref(), Some("team chat"));
    assert_eq!(history[0].context.as_deref(), Some("standup notes"));
}

/// Failure records are immutable per `(bank, task_id, attempt_n)` — there is
/// no version chain and no prior value — so the attempt's audit row records
/// only the context written by that attempt, and change detection compares
/// hashes across successive attempt rows.
#[test]
fn sqlite_failure_audit_row_carries_context_stats() {
    let store = sqlite_store();
    let bank = unique("sqlite-audit-fail");
    let with_item = unique("i-fail-ctx");
    store
        .commit_failure_record(
            &carrier(
                &with_item,
                &bank,
                EpisodicType::Failure,
                Some("attempt 1 of 3"),
            ),
            &failure_draft(&bank, &with_item, "audit-task", 1, Some("attempt 1 of 3")),
            "actor",
        )
        .expect("failure with context");
    let bare_item = unique("i-fail-bare");
    store
        .commit_failure_record(
            &carrier(&bare_item, &bank, EpisodicType::Failure, None),
            &failure_draft(&bank, &bare_item, "audit-task", 2, None),
            "actor",
        )
        .expect("failure without context");

    let detail = detail_of(&store, &bank, &with_item, "failure_record");
    assert_eq!(detail["record"], json!("failure"));
    assert_eq!(detail["context_present"], json!(true));
    assert_eq!(detail["context_len"], json!("attempt 1 of 3".len()));
    assert_eq!(
        detail["context_hash"],
        json!(sha256_hex(CTX_HASH_DOMAIN, "attempt 1 of 3"))
    );
    assert!(
        !serde_json::to_string(&detail)
            .unwrap()
            .contains("attempt 1"),
        "raw context must stay out: {detail}"
    );
    let bare = detail_of(&store, &bank, &bare_item, "failure_record");
    assert_eq!(bare["context_present"], json!(false));
    assert_eq!(bare["context_len"], json!(0));
    assert!(bare.get("context_hash").is_none(), "{bare}");
}

/// Triple supersede: each edge's audit row carries its own context stats, a
/// supersede with changed context produces a different hash, raw values stay
/// out, and the superseded edge keeps its sealed context readable through the
/// triple version chain (`triple_history`).
#[test]
fn sqlite_triple_audit_rows_carry_context_stats() {
    let store = sqlite_store();
    let bank = unique("sqlite-audit-triple");
    let i1 = unique("i-triple-v1");
    let i2 = unique("i-triple-v2");
    store
        .commit_triple_add(
            &carrier(&i1, &bank, EpisodicType::Triple, Some("Q1 review notes")),
            &edge("e1", &bank, &i1, "Boston"),
            true,
            "actor",
        )
        .expect("edge v1");
    store
        .commit_triple_add(
            &carrier(&i2, &bank, EpisodicType::Triple, Some("Q2 review notes")),
            &edge(&unique("e2"), &bank, &i2, "NYC"),
            true,
            "actor",
        )
        .expect("edge v2 supersedes");

    let v1 = detail_of(&store, &bank, &i1, "triple_add");
    let v2 = detail_of(&store, &bank, &i2, "triple_add");
    assert_eq!(v1["context_present"], json!(true));
    assert_eq!(
        v1["context_hash"],
        json!(sha256_hex(CTX_HASH_DOMAIN, "Q1 review notes"))
    );
    assert_eq!(
        v2["context_hash"],
        json!(sha256_hex(CTX_HASH_DOMAIN, "Q2 review notes"))
    );
    assert_ne!(v1["context_hash"], v2["context_hash"], "context changed");
    assert!(
        !serde_json::to_string(&[&v1, &v2])
            .unwrap()
            .contains("review notes"),
        "raw context must stay out"
    );
    let history = store
        .triple_history(&bank, "Alice", "lives_in", 10)
        .expect("version chain");
    assert_eq!(history.len(), 2);
    assert_eq!(
        history
            .iter()
            .find(|t| t.id == "e1")
            .and_then(|t| t.context.clone())
            .as_deref(),
        Some("Q1 review notes")
    );
}

#[test]
fn pg_history_audit_rows_carry_context_stats() {
    let _guard = lock_pg_test();
    let url = postgres_url_from_env();
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    let store = match PostgresStore::open(url.as_str(), kms) {
        Ok(store) => store,
        Err(err) => {
            eprintln!(
                "{}",
                crate::pg_skip::pg_skip_note("postgres history audit tests", &err)
            );
            return;
        }
    };
    let bank = unique("pg-audit-task");
    let v1_item = unique("pg-i-task-v1");
    let v2_item = unique("pg-i-task-v2");
    store
        .commit_task_upsert(
            &carrier(&v1_item, &bank, EpisodicType::Task, Some("team chat")),
            &task_draft(&bank, &v1_item, "pg-audit-task", Some("team chat")),
            "actor",
        )
        .expect("task v1");
    store
        .commit_task_upsert(
            &carrier(&v2_item, &bank, EpisodicType::Task, Some("standup notes")),
            &task_draft(&bank, &v2_item, "pg-audit-task", Some("standup notes")),
            "actor",
        )
        .expect("task v2");
    let v1 = detail_of(&store, &bank, &v1_item, "task_upsert");
    let v2 = detail_of(&store, &bank, &v2_item, "task_upsert");
    assert_eq!(
        v1["context_hash"],
        json!(sha256_hex(CTX_HASH_DOMAIN, "team chat"))
    );
    assert_eq!(
        v2["context_hash"],
        json!(sha256_hex(CTX_HASH_DOMAIN, "standup notes"))
    );
    assert_ne!(v1["context_hash"], v2["context_hash"], "context changed");
    assert!(
        !serde_json::to_string(&[&v1, &v2])
            .unwrap()
            .contains("team chat"),
        "raw context must stay out"
    );
    let history = store
        .task_history(&bank, "pg-audit-task", 10)
        .expect("history");
    assert_eq!(history[1].context.as_deref(), Some("team chat"));
}

#[test]
fn pg_triple_audit_rows_carry_context_stats() {
    let _guard = lock_pg_test();
    let url = postgres_url_from_env();
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    let store = match PostgresStore::open(url.as_str(), kms) {
        Ok(store) => store,
        Err(err) => {
            eprintln!(
                "{}",
                crate::pg_skip::pg_skip_note("postgres triple audit tests", &err)
            );
            return;
        }
    };
    let bank = unique("pg-audit-triple");
    let i1 = unique("pg-i-triple-v1");
    let i2 = unique("pg-i-triple-v2");
    let e1 = unique("pg-e1");
    let e2 = unique("pg-e2");
    store
        .commit_triple_add(
            &carrier(&i1, &bank, EpisodicType::Triple, Some("Q1 review notes")),
            &edge(&e1, &bank, &i1, "Boston"),
            true,
            "actor",
        )
        .expect("edge v1");
    store
        .commit_triple_add(
            &carrier(&i2, &bank, EpisodicType::Triple, Some("Q2 review notes")),
            &edge(&e2, &bank, &i2, "NYC"),
            true,
            "actor",
        )
        .expect("edge v2 supersedes");
    let v1 = detail_of(&store, &bank, &i1, "triple_add");
    let v2 = detail_of(&store, &bank, &i2, "triple_add");
    assert_eq!(
        v1["context_hash"],
        json!(sha256_hex(CTX_HASH_DOMAIN, "Q1 review notes"))
    );
    assert_eq!(
        v2["context_hash"],
        json!(sha256_hex(CTX_HASH_DOMAIN, "Q2 review notes"))
    );
    assert_ne!(v1["context_hash"], v2["context_hash"], "context changed");
    assert!(
        !serde_json::to_string(&[&v1, &v2])
            .unwrap()
            .contains("review notes"),
        "raw context must stay out"
    );
    let history = store
        .triple_history(&bank, "Alice", "lives_in", 10)
        .expect("version chain");
    assert_eq!(history.len(), 2);
    assert_eq!(
        history
            .iter()
            .find(|t| t.id == e1)
            .and_then(|t| t.context.clone())
            .as_deref(),
        Some("Q1 review notes")
    );
}
