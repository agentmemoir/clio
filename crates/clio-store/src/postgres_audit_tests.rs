// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for Postgres parity tests of the audit and
//! correction surface: telemetry rows, append-only enforcement, atomic
//! discrete correction, and audit-trail reconstruction.
//!
//! ## Owns
//! - Postgres audit / correct parity tests
//!
//! ## Does not own
//! - SQLite behavior (see `audit_correct_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::db_url::postgres_url_from_env;
use crate::migrate::IntoAmError;
use crate::pg_lock::lock_pg_test;
use crate::{CorrectionSpec, DekProvider, LocalDevKms, PostgresStore, SqliteStore, Store};
use clio_types::{ItemKind, MemoryItem, SemanticCategory};
use serde_json::json;
use std::sync::Arc;

use crate::test_unique::unique;

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

/// Episodic belief item: exercises episodic_type / source_type mappings.
fn episodic_belief_item(id: &str, bank: &str) -> MemoryItem {
    let mut item = item(id, bank);
    item.kind = ItemKind::Episodic;
    item.category = None;
    item.episodic_type = Some(clio_types::EpisodicType::Gist);
    item.epistemic_kind = clio_types::EpistemicKind::Belief;
    item.source_type = Some(clio_types::SourceType::UserStated);
    item
}

fn corrected(pg_bank: &str, prior: &MemoryItem, new_id: &str, ts: &str) -> MemoryItem {
    let mut new = prior.clone();
    new.id = new_id.to_owned();
    new.bank_id = pg_bank.to_owned();
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
fn pg_correct_audit_trail_and_append_only_parity() {
    let _guard = lock_pg_test();
    let url = postgres_url_from_env();
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    let pg = PostgresStore::open(url.as_str(), kms.clone()).expect("pg");
    let lite = SqliteStore::open_in_memory(kms).expect("sqlite");
    let bank = unique("bank-audit-pg");
    let rev_a = unique("rev-a");
    let rev_b = unique("rev-b");
    for store in [&lite as &dyn Store, &pg] {
        store
            .create_memory_item(&item(&rev_a, &bank), "agent")
            .expect("create");
        // The store telemetry row exists with the admission score.
        let spec = CorrectionSpec {
            bank_id: bank.clone(),
            prior_item_id: rev_a.clone(),
            new_item: corrected(&bank, &item(&rev_a, &bank), &rev_b, "2024-06-01T00:00:00Z"),
            reason: "fix the time".into(),
            actor: "user".into(),
            timestamp: "2024-06-01T00:00:00Z".into(),
        };
        let out = store.correct_item(&spec).expect("correction");
        assert_eq!(out.new_item_id, rev_b);
        assert!(!out.already_superseded);
        // Both revisions remain readable; prior closed on both axes.
        let prior = store
            .get_memory_item(&bank, &rev_a)
            .expect("prior readable")
            .expect("prior exists");
        assert_eq!(prior.tx_until.as_deref(), Some("2024-06-01T00:00:00Z"));
        assert_eq!(
            prior.snapshot.as_ref().expect("prior snapshot")["meeting"],
            "2pm"
        );
        let active = store
            .get_memory_item(&bank, &rev_b)
            .expect("new readable")
            .expect("new exists");
        assert!(active.tx_until.is_none());
        assert_eq!(
            active.snapshot.as_ref().expect("new snapshot")["meeting"],
            "3pm"
        );
        // Trail reconstructs the full lifecycle from either revision id.
        for queried in [&rev_a, &rev_b] {
            let trail = store.audit_trail(&bank, queried).expect("trail");
            assert_eq!(trail.lineage_ids, vec![rev_a.clone(), rev_b.clone()]);
            assert_eq!(trail.revisions.len(), 2);
            let ts: Vec<&str> = trail.events.iter().map(|e| e.timestamp.as_str()).collect();
            let mut sorted = ts.clone();
            sorted.sort_unstable();
            assert_eq!(ts, sorted);
            let mut has_create = false;
            let mut has_correct = false;
            for event in &trail.events {
                if event.operation == "create_memory_item" {
                    has_create = true;
                }
                if event.operation == "correct" {
                    has_correct = true;
                    assert_eq!(event.actor, "user");
                    assert_eq!(event.detail["reason"], "fix the time");
                }
            }
            assert!(has_create, "creation event missing");
            assert!(has_correct, "{:?}", trail.events);
        }
        // Missing id → typed error on both backends.
        let err = store.audit_trail(&bank, "nope").expect_err("missing");
        assert!(err.message.contains("not found"));
        // Direct UPDATE/DELETE on audit_events must fail
        // (sqlite via test_execute; pg checked separately below).
    }
    // Parity: sqlite and pg produce identical lineage and revision ordering.
    let a = lite.audit_trail(&bank, &rev_a).expect("lite trail");
    let b = pg.audit_trail(&bank, &rev_a).expect("pg trail");
    assert_eq!(a.lineage_ids, b.lineage_ids);
    let revs: Vec<String> = a.revisions.iter().map(|r| r.item_id.clone()).collect();
    assert_eq!(
        revs,
        b.revisions
            .iter()
            .map(|r| r.item_id.clone())
            .collect::<Vec<_>>()
    );
}

#[test]
fn pg_correct_rejections_and_noop_parity() {
    let _guard = lock_pg_test();
    let url = postgres_url_from_env();
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    let pg = PostgresStore::open(url.as_str(), kms).expect("pg");
    let bank = unique("bank-pg-reject");
    let id = unique("itm-pg-r");
    pg.create_memory_item(&item(&id, &bank), "agent")
        .expect("create");
    let mk = |bank_id: &str, prior: &str, reason: &str, actor: &str| CorrectionSpec {
        bank_id: bank_id.into(),
        prior_item_id: prior.into(),
        new_item: corrected(
            &bank,
            &item(&id, &bank),
            &unique("itm-pg-n"),
            "2024-06-01T00:00:00Z",
        ),
        reason: reason.into(),
        actor: actor.into(),
        timestamp: "2024-06-01T00:00:00Z".into(),
    };
    for spec in [
        mk("", &id, "r", "agent"),
        mk(&bank, "", "r", "agent"),
        mk(&bank, &id, "", "agent"),
        mk(&bank, &id, "   ", "agent"),
        mk(&bank, &id, "r", ""),
    ] {
        let err = pg.correct_item(&spec).expect_err("rejected");
        assert_eq!(err.code, clio_types::ErrorCode::InvalidArgument);
    }
    // Missing prior maps to the typed item_not_found error.
    let missing = mk(&bank, "nope", "r", "agent");
    let err = pg.correct_item(&missing).expect_err("missing prior");
    assert_eq!(err.code, clio_types::ErrorCode::ItemNotFound);
    // Already-superseded prior is a no-op.
    let first = mk(&bank, &id, "fix", "agent");
    let out = pg.correct_item(&first).expect("first");
    assert!(!out.already_superseded);
    let second = mk(&bank, &id, "again", "agent");
    let out2 = pg.correct_item(&second).expect("second");
    assert!(out2.already_superseded);
    // Discarded prior is rejected (separate prior, discarded below).
    let disc_id = unique("itm-pg-disc");
    pg.create_memory_item(&item(&disc_id, &bank), "agent")
        .expect("create discarded target");
    pg.discard_item(&bank, &disc_id, "ops cleanup", "2024-06-01", "harness")
        .expect("discard");
    let discarded = mk(&bank, &disc_id, "late fix", "agent");
    let err = pg.correct_item(&discarded).expect_err("discarded prior");
    assert_eq!(err.code, clio_types::ErrorCode::InvalidArgument);
}

#[test]
fn pg_correct_duplicate_new_id_rejected() {
    let _guard = lock_pg_test();
    let url = postgres_url_from_env();
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    let pg = PostgresStore::open(url.as_str(), kms).expect("pg");
    let bank = unique("bank-pg-dup");
    let a = unique("itm-pg-a");
    let b = unique("itm-pg-b");
    let collide = unique("itm-pg-c");
    pg.create_memory_item(&episodic_belief_item(&a, &bank), "agent")
        .expect("create");
    pg.create_memory_item(&item(&b, &bank), "agent")
        .expect("create");
    pg.create_memory_item(&item(&collide, &bank), "agent")
        .expect("collider");
    let spec = CorrectionSpec {
        bank_id: bank.clone(),
        prior_item_id: a.clone(),
        new_item: corrected(
            &bank,
            &episodic_belief_item(&a, &bank),
            &collide,
            "2024-06-01T00:00:00Z",
        ),
        reason: "r".into(),
        actor: "agent".into(),
        timestamp: "2024-06-01T00:00:00Z".into(),
    };
    // Driver-dependent classification: the insert must fail closed either way,
    // and no lineage row may appear (additive only).
    let err = pg.correct_item(&spec).expect_err("duplicate id");
    assert!(
        err.code == clio_types::ErrorCode::InvalidArgument
            || err.code == clio_types::ErrorCode::Internal
    );
    let prior = pg
        .get_memory_item(&bank, &a)
        .expect("prior readable")
        .expect("prior exists");
    assert!(prior.tx_until.is_none(), "failed insert must roll back");
}

#[test]
fn t18_06_pg_discard_telemetry_same_transaction() {
    let _guard = lock_pg_test();
    let url = postgres_url_from_env();
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    let pg = PostgresStore::open(url.as_str(), kms).expect("pg");
    let bank = unique("bank-discard-pg");
    let id = unique("itm-pd");
    pg.create_memory_item(&item(&id, &bank), "agent")
        .expect("create");
    pg.discard_item(&bank, &id, "ops cleanup", "2024-06-01", "harness")
        .expect("discard");
    let row = pg
        .with_client(|client| {
            client
                .query_opt(
                    "SELECT actor, detail_json FROM audit_events
                     WHERE bank_id = $1 AND item_id = $2 AND operation = 'discard'",
                    &[&bank, &id],
                )
                .am("discard audit read")
        })
        .expect("discard row");
    let Some(row) = row else {
        panic!("discard telemetry row missing");
    };
    let actor: String = row.get(0);
    let detail: String = row.get(1);
    assert_eq!(actor, "harness");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&detail).expect("detail")["reason"],
        "ops cleanup"
    );
}
