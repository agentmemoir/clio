// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the audit/correction conformance suite on
//! the SQLite backend: telemetry emission on mutations, append-only
//! enforcement, bi-temporal correction, and audit-trail reconstruction.
//!
//! ## Owns
//! - SQLite audit / correct behavioral tests
//!
//! ## Does not own
//! - Production SQL (`sqlite_correct` / `sqlite_audit`)
//! - Postgres parity (see `pg_audit_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::migrate::IntoAmError;
use crate::{ContinuousStore, CorrectionSpec, LocalDevKms, SqliteStore, Store};
use clio_types::{ErrorCode, ItemKind, MemoryItem, SemanticCategory};
use serde_json::json;
use std::sync::Arc;

fn item(id: &str, bank: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: "subj".into(),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::ToolConfig),
        episodic_type: None,
        epistemic_kind: clio_types::EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.7),
        admission_score: Some(0.88),
        source_ref: Some("turn-3".into()),
        snapshot: Some(json!({"meeting": "2pm"})),
        gist: Some("meeting at 2pm".into()),
        valid_from: "2024-01-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2024-01-01T00:00:00Z".into(),
        tx_until: None,
        created_at: "2024-01-01T00:00:00Z".into(),
        updated_at: "2024-01-01T00:00:00Z".into(),
        context: None,
    }
}

fn store() -> SqliteStore {
    SqliteStore::open_in_memory(Arc::new(LocalDevKms::new())).expect("sqlite store")
}

fn corrected(spec_bank: &str, prior: &MemoryItem, new_id: &str, ts: &str) -> MemoryItem {
    let mut new = prior.clone();
    new.id = new_id.to_owned();
    new.bank_id = spec_bank.to_owned();
    new.snapshot = Some(json!({"meeting": "3pm"}));
    new.gist = Some("meeting at 3pm".into());
    new.valid_from = ts.to_owned();
    new.valid_until = None;
    new.tx_from = ts.to_owned();
    new.tx_until = None;
    new.created_at = ts.to_owned();
    new.updated_at = ts.to_owned();
    new
}

#[test]
fn t18_01_store_and_discard_emit_telemetry() {
    let s = store();
    let bank = "bank-tel";
    s.create_memory_item(&item("itm-1", bank), "agent")
        .expect("create");
    let rows = query_ops(&s, "itm-1");
    assert!(
        rows.contains(&("create_memory_item".to_owned(), "agent".to_owned())),
        "{rows:?}"
    );
    s.discard_item(bank, "itm-1", "ops cleanup", "2024-06-01", "harness")
        .expect("discard");
    let rows = query_ops(&s, "itm-1");
    assert!(
        rows.contains(&("discard".to_owned(), "harness".to_owned())),
        "{rows:?}"
    );
}

#[test]
fn t18_02_invalidate_emits_telemetry_and_trail_orders_timeline() {
    let s = store();
    let bank = "bank-inv";
    s.create_memory_item(&item("itm-1", bank), "agent")
        .expect("create");
    s.invalidate_item(bank, "itm-1", "2024-06-01", "2024-06-01", None, "agent")
        .expect("invalidate");
    let trail = s.audit_trail(bank, "itm-1").expect("trail");
    let seen = event_ops(&trail);
    assert!(seen.contains(&"create_memory_item"), "{seen:?}");
    assert!(seen.contains(&"invalidate"), "{seen:?}");
    // Deterministic ascending order: creation precedes invalidation.
    assert_ascending(&trail);
}

fn event_ops(trail: &crate::ItemAuditTrail) -> Vec<&'static str> {
    trail
        .events
        .iter()
        .map(|e| match e.operation.as_str() {
            "create_memory_item" => "create_memory_item",
            "update_memory_item" => "update_memory_item",
            "invalidate" => "invalidate",
            "discard" => "discard",
            "correct" => "correct",
            "belief_observe" => "belief_observe",
            _ => "other",
        })
        .collect()
}

fn assert_ascending(trail: &crate::ItemAuditTrail) {
    let ts: Vec<&str> = trail.events.iter().map(|e| e.timestamp.as_str()).collect();
    let mut sorted = ts.clone();
    sorted.sort_unstable();
    assert_eq!(ts, sorted, "events must be ascending by (created_at, id)");
}

fn query_ops(s: &SqliteStore, item_id: &str) -> Vec<(String, String)> {
    s.with_conn(|conn| {
        let mut stmt = conn
            .prepare(
                "SELECT operation, actor FROM audit_events
                 WHERE item_id = ?1 ORDER BY created_at, id",
            )
            .expect("prepare");
        let mapped = stmt
            .query_map([item_id], |r| Ok((r.get(0)?, r.get(1)?)))
            .expect("query");
        Ok(mapped.collect::<Result<Vec<_>, _>>().expect("collect rows"))
    })
    .expect("audit rows")
}

#[test]
fn t18_03_t18_04_discrete_correction_keeps_history_queryable() {
    let s = store();
    let bank = "bank-correct";
    s.create_memory_item(&item("itm-old", bank), "agent")
        .expect("create");
    let spec = CorrectionSpec {
        bank_id: bank.into(),
        prior_item_id: "itm-old".into(),
        new_item: corrected(
            bank,
            &item("itm-old", bank),
            "itm-new",
            "2024-06-01T00:00:00Z",
        ),
        reason: "wrong meeting time".into(),
        actor: "user".into(),
        timestamp: "2024-06-01T00:00:00Z".into(),
    };
    let out = s.correct_item(&spec).expect("correct");
    assert!(!out.already_superseded);
    assert_eq!(out.new_item_id, "itm-new");
    // Prior assertion closed on both axes, retained as past system belief.
    let prior = s
        .get_memory_item(bank, "itm-old")
        .expect("prior readable")
        .expect("prior exists");
    assert_eq!(prior.tx_until.as_deref(), Some("2024-06-01T00:00:00Z"));
    assert_eq!(prior.valid_until.as_deref(), Some("2024-06-01T00:00:00Z"));
    assert_eq!(
        prior.snapshot.as_ref().expect("prior snapshot")["meeting"],
        "2pm"
    );
    // New assertion active, linked both ways, create-time score untouched.
    let active = s
        .get_memory_item(bank, "itm-new")
        .expect("new readable")
        .expect("new exists");
    assert!(active.tx_until.is_none());
    assert_eq!(
        active.snapshot.as_ref().expect("new snapshot")["meeting"],
        "3pm"
    );
    assert_eq!(active.admission_score, Some(0.88));
    // Telemetry rows carry the reason and actor on both lineage ids.
    for (target, expected_detail_key) in
        [("itm-old", "corrected_by"), ("itm-new", "corrects_item_id")]
    {
        let detail: String = query_detail(&s, target);
        let detail: serde_json::Value = serde_json::from_str(&detail).expect("detail json");
        assert_eq!(detail["reason"], "wrong meeting time");
        assert!(detail[expected_detail_key].is_string());
    }
    // No physical delete ever happened: both rows still exist.
    assert!(s.get_memory_item(bank, "itm-old").expect("get").is_some());
}

#[test]
fn t18_02_audit_trail_traverses_lineage_from_any_revision() {
    let s = store();
    let bank = "bank-lineage";
    s.create_memory_item(&item("rev-1", bank), "agent")
        .expect("create");
    let mk = |prior_id: &str, new_id: &str, ts: &str| CorrectionSpec {
        bank_id: bank.into(),
        prior_item_id: prior_id.into(),
        new_item: corrected(bank, &item(prior_id, bank), new_id, ts),
        reason: "chained correction".into(),
        actor: "agent".into(),
        timestamp: ts.into(),
    };
    s.correct_item(&mk("rev-1", "rev-2", "2024-02-01T00:00:00Z"))
        .expect("first correction");
    s.correct_item(&mk("rev-2", "rev-3", "2024-03-01T00:00:00Z"))
        .expect("second correction");
    for queried in ["rev-1", "rev-2", "rev-3"] {
        let trail = s.audit_trail(bank, queried).expect("trail");
        assert_eq!(
            trail.lineage_ids,
            vec!["rev-1".to_owned(), "rev-2".to_owned(), "rev-3".to_owned()],
            "trail from {queried}"
        );
        assert_eq!(trail.revisions.len(), 3);
        // Every event across the lineage is present, ascending.
        assert_ascending(&trail);
        assert!(event_ops(&trail).contains(&"correct"));
    }
    // Missing id → typed error.
    let err = s.audit_trail(bank, "nope").expect_err("missing");
    assert_eq!(err.code, ErrorCode::ItemNotFound);
}

#[test]
fn t18_06_audit_events_reject_update_and_delete() {
    let s = store();
    let bank = "bank-immutable";
    s.create_memory_item(&item("itm-i", bank), "agent")
        .expect("create");
    for sql in [
        "UPDATE audit_events SET actor = 'forged'",
        "DELETE FROM audit_events",
    ] {
        let err = s.test_execute(sql).expect_err("append-only enforcement");
        assert!(err.message.contains("append-only"), "{err:?}");
    }
}

fn query_detail(s: &SqliteStore, item_id: &str) -> String {
    s.with_conn(|conn| {
        Ok(conn
            .query_row(
                "SELECT detail_json FROM audit_events
                 WHERE item_id = ?1 AND operation = 'correct' LIMIT 1",
                [item_id],
                |r| r.get(0),
            )
            .expect("correct audit row"))
    })
    .expect("query_detail")
}

#[test]
fn discard_audit_row_carries_reason_and_actor() {
    let s = store();
    let bank = "bank-discard-tel";
    s.create_memory_item(&item("itm-d", bank), "agent")
        .expect("create");
    s.discard_item(bank, "itm-d", "ops cleanup", "2024-06-01", "harness")
        .expect("discard");
    let (actor, detail): (String, String) = s
        .with_conn(|conn| {
            conn.query_row(
                "SELECT actor, detail_json FROM audit_events
                 WHERE item_id = 'itm-d' AND operation = 'discard'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .am("discard row read")
        })
        .expect("discard row");
    assert_eq!(actor, "harness");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&detail).expect("detail")["reason"],
        "ops cleanup"
    );
}

#[test]
fn invalidate_sets_forward_pointer_and_row_actor() {
    let s = store();
    let bank = "bank-inv";
    s.create_memory_item(&item("itm-2", bank), "agent")
        .expect("create");
    let out = s
        .invalidate_item(
            bank,
            "itm-2",
            "2024-06-01",
            "2024-06-01",
            Some("itm-3"),
            "agent",
        )
        .expect("invalidate");
    assert_eq!(out.closed_ids, vec!["itm-2".to_owned()]);
    let superseded: Option<String> = s
        .with_conn(|conn| {
            conn.query_row(
                "SELECT superseded_by_id FROM items WHERE id = 'itm-2'",
                [],
                |r| r.get(0),
            )
            .am("prior row read")
        })
        .expect("prior row");
    assert_eq!(superseded.as_deref(), Some("itm-3"));
    let (actor, detail): (String, String) = s
        .with_conn(|conn| {
            conn.query_row(
                "SELECT actor, detail_json FROM audit_events
                 WHERE item_id = 'itm-2' AND operation = 'invalidate'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .am("invalidate audit read")
        })
        .expect("invalidate row");
    assert_eq!(actor, "agent");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&detail).expect("detail")["replacement_id"],
        "itm-3"
    );
}

#[test]
fn continuous_correction_emits_correct_operation() {
    let s = store();
    let bank = "bank-cont-tel";
    s.put_attribute_schema(
        &clio_types::AttributeSchema {
            bank_id: bank.into(),
            attr_key: "affinity".into(),
            update_rule: clio_types::UpdateRule::Continuous,
            alpha: Some(0.5),
            trend_window: None,
            bounds_lo: None,
            bounds_hi: None,
            clamp_values: false,
            created_at: "2024-01-01T00:00:00Z".into(),
            updated_at: None,
        },
        "agent",
    )
    .expect("schema");
    // Reasoned observation = correction: operation `correct` carries the reason.
    s.observe_continuous_with_reason(
        bank,
        "affinity",
        0.4,
        "2024-01-02T00:00:00Z",
        "operator",
        Some("adjusted"),
    )
    .expect("continuous correction");
    let (operation, actor, detail): (String, String, String) = s
        .with_conn(|conn| {
            conn.query_row(
                "SELECT operation, actor, detail_json FROM audit_events
                 WHERE operation = 'correct' AND detail_json LIKE '%affinity%'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .am("continuous correct row")
        })
        .expect("row");
    assert_eq!(operation, "correct");
    assert_eq!(actor, "operator");
    let detail: serde_json::Value = serde_json::from_str(&detail).expect("detail");
    assert_eq!(detail["reason"], "adjusted");
    assert_eq!(detail["attr_key"], "affinity");
    // A plain observation (no reason) stays `continuous_observe`.
    s.observe_continuous(bank, "affinity", 0.5, "2024-01-03T00:00:00Z", "agent")
        .expect("plain observation");
    let observed: i64 = s
        .with_conn(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM audit_events
                 WHERE operation = 'continuous_observe' AND detail_json LIKE '%affinity%'",
                [],
                |r| r.get(0),
            )
            .am("plain row")
        })
        .expect("plain count");
    assert_eq!(observed, 1);
}

#[test]
fn t18_01_create_and_invalidate_lineage_query_helper() {
    let s = store();
    let bank = "bank-helper";
    s.create_memory_item(&item("itm-h", bank), "agent")
        .expect("create");
    let rows = query_ops(&s, "itm-h");
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0],
        ("create_memory_item".to_owned(), "agent".to_owned())
    );
}
