// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for correction rejection/no-op conformance tests
//! on the SQLite backend (fail-closed validation, duplicate ids, lineage no-op).
//!
//! ## Owns
//! - SQLite correct-rejection behavioral tests
//!
//! ## Does not own
//! - Happy-path correction/audit tests (`audit_correct_tests`)
//! - Production SQL (`sqlite_correct`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::{CorrectionSpec, LocalDevKms, SqliteStore, Store};
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
fn correct_rejections_fail_closed() {
    let s = store();
    let bank = "bank-reject";
    s.create_memory_item(&item("itm-r", bank), "agent")
        .expect("create");
    let mk = |bank_id: &str, prior: &str, reason: &str, actor: &str| CorrectionSpec {
        bank_id: bank_id.into(),
        prior_item_id: prior.into(),
        new_item: corrected(bank, &item("itm-r", bank), "itm-r2", "2024-06-01T00:00:00Z"),
        reason: reason.into(),
        actor: actor.into(),
        timestamp: "2024-06-01T00:00:00Z".into(),
    };
    for spec in [
        mk("", "itm-r", "r", "agent"),
        mk(bank, "", "r", "agent"),
        mk(bank, "itm-r", "", "agent"),
        mk(bank, "itm-r", "   ", "agent"),
        mk(bank, "itm-r", "r", ""),
    ] {
        let err = s.correct_item(&spec).expect_err("rejected");
        assert_eq!(err.code, ErrorCode::InvalidArgument);
    }
    // Discarded prior is rejected.
    s.discard_item(bank, "itm-r", "ops cleanup", "2024-06-01", "harness")
        .expect("discard");
    let spec = mk(bank, "itm-r", "late fix", "agent");
    let err = s.correct_item(&spec).expect_err("discarded prior");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
}

#[test]
fn correct_duplicate_new_id_rejected() {
    let s = store();
    let bank = "bank-dup";
    s.create_memory_item(&item("itm-a", bank), "agent")
        .expect("create");
    s.create_memory_item(&item("itm-b", bank), "agent")
        .expect("create");
    let spec = CorrectionSpec {
        bank_id: bank.into(),
        prior_item_id: "itm-a".into(),
        new_item: corrected(
            bank,
            &item("itm-a", bank),
            "itm-b-exists",
            "2024-06-01T00:00:00Z",
        ),
        reason: "r".into(),
        actor: "agent".into(),
        timestamp: "2024-06-01T00:00:00Z".into(),
    };
    // The corrected id collides with an existing row (pre-created below), so
    // the insert fails closed instead of overwriting (additive only).
    let mut collide = item("itm-b-exists", bank);
    collide.admission_score = Some(0.9);
    s.create_memory_item(&collide, "agent").expect("collider");
    let err = s.correct_item(&spec).expect_err("duplicate id");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert_eq!(err.message, "duplicate item id");
}

#[test]
fn correct_episodic_belief_item_carries_source_type() {
    let s = store();
    let bank = "bank-bel";
    let mut belief = item("itm-bel", bank);
    belief.kind = ItemKind::Episodic;
    belief.category = None;
    belief.episodic_type = Some(clio_types::EpisodicType::Gist);
    belief.epistemic_kind = clio_types::EpistemicKind::Belief;
    belief.source_type = Some(clio_types::SourceType::UserStated);
    s.create_memory_item(&belief, "agent").expect("create");
    let spec = CorrectionSpec {
        bank_id: bank.into(),
        prior_item_id: "itm-bel".into(),
        new_item: corrected(bank, &belief, "itm-bel2", "2024-06-01T00:00:00Z"),
        reason: "fixed".into(),
        actor: "agent".into(),
        timestamp: "2024-06-01T00:00:00Z".into(),
    };
    let out = s.correct_item(&spec).expect("correction");
    assert_eq!(out.new_item_id, "itm-bel2");
    let active = s
        .get_memory_item(bank, "itm-bel2")
        .expect("readable")
        .expect("exists");
    assert_eq!(active.epistemic_kind, clio_types::EpistemicKind::Belief);
    assert_eq!(active.source_type, Some(clio_types::SourceType::UserStated));
}

#[test]
fn correct_already_superseded_prior_is_noop() {
    let s = store();
    let bank = "bank-noop";
    s.create_memory_item(&item("itm-n", bank), "agent")
        .expect("create");
    let mk = |prior: &str, new_id: &str| CorrectionSpec {
        bank_id: bank.into(),
        prior_item_id: prior.into(),
        new_item: corrected(bank, &item("itm-n", bank), new_id, "2024-06-01T00:00:00Z"),
        reason: "r".into(),
        actor: "agent".into(),
        timestamp: "2024-06-01T00:00:00Z".into(),
    };
    s.correct_item(&mk("itm-n", "itm-n2")).expect("first");
    // Correcting the now-superseded prior again is a no-op with no new row.
    let second = s.correct_item(&mk("itm-n", "itm-z")).expect("second");
    assert!(second.already_superseded);
    assert!(
        s.get_memory_item(bank, "itm-z")
            .expect("no new row")
            .is_none()
    );
}

#[test]
fn correct_missing_item_is_item_not_found() {
    let s = store();
    let spec = CorrectionSpec {
        bank_id: "bank-x".into(),
        prior_item_id: "nope".into(),
        new_item: item("itm-new", "bank-x"),
        reason: "r".into(),
        actor: "agent".into(),
        timestamp: "2024-06-01T00:00:00Z".into(),
    };
    let err = s.correct_item(&spec).expect_err("missing prior");
    assert_eq!(err.code, ErrorCode::ItemNotFound);
}
