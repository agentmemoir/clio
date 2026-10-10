// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests `hygiene_clean` batch semantics (`clean.rs`): the
//! applied-vs-pending batch abort, duplicate target item ids writing
//! collision-free log rows, and the preflight reason gate when `keep`
//! resolves a target to discard.
//!
//! ## Owns
//! - Test coverage for batch abort (batch clause), duplicate-target
//!   log-row uniqueness, and the confirmed-`keep` preflight rejection
//!
//! ## Does not own
//! - The batch/log behavior itself (`clean.rs` and the store backends)
//! - Store SQL or MCP dispatch
//!
//! ## Boundary
//!
//! Tests here break when clean batch semantics change; they never change it.
//!
//! Keep this module focused on `hygiene_clean` batch tests.

use crate::clean::{CleanAction, CleanTarget, clean};
use clio_store::{HygieneStore, LocalDevKms, SqliteStore, Store};
use clio_types::{Actor, Bank, EpistemicKind, ItemKind, MemoryItem, OpContext, SemanticCategory};
use std::sync::Arc;

fn store() -> Arc<SqliteStore> {
    SqliteStore::open_in_memory(Arc::new(LocalDevKms::new()))
        .map(Arc::new)
        .expect("open sqlite")
}

fn item(id: &str, bank: &str, gist: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: format!("subj-{id}"),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::ToolConfig),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.8),
        admission_score: Some(0.5),
        source_ref: Some("turn_9".into()),
        snapshot: None,
        gist: Some(gist.into()),
        valid_from: "2026-06-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2026-06-01T00:00:00Z".into(),
        tx_until: None,
        created_at: "2026-06-01T00:00:00Z".into(),
        updated_at: "2026-06-01T00:00:00Z".into(),
        context: None,
    }
}

fn ctx(bank: &str) -> OpContext {
    OpContext::new(Bank::new(bank), Actor::Agent)
}

fn target(id: &str) -> CleanTarget {
    CleanTarget {
        item_id: id.into(),
        noise_score: Some(0.75),
        suggested_action: Some("archive".into()),
    }
}

fn args<'a>(
    store: &'a SqliteStore,
    opctx: &'a OpContext,
    targets: &'a [CleanTarget],
    action: CleanAction,
    confirm: bool,
) -> crate::clean::CleanArgs<'a> {
    crate::clean::CleanArgs {
        store,
        hygiene: store,
        ctx: opctx,
        targets,
        action,
        confirm,
        reason: "hygiene noise cleanup",
        timestamp: "2026-09-20T00:00:00Z",
        batch_id: "batch-1",
        note: Some("review me"),
    }
}

#[test]
fn batch_aborts_on_first_failure_reporting_applied_vs_pending() {
    let s = store();
    let bank = "bank-abort";
    let s_ref = &*s as &dyn Store;
    s_ref
        .create_memory_item(&item("itm-1", bank, "first"), "actor")
        .unwrap();
    // itm-missing does not exist → its action fails; itm-3 must stay pending.
    let targets = vec![target("itm-1"), target("itm-missing"), target("itm-3")];
    let outcome = clean(&args(&s, &ctx(bank), &targets, CleanAction::Archive, true)).unwrap();
    assert_eq!(outcome.applied, vec!["itm-1".to_owned()]);
    assert_eq!(outcome.pending, vec!["itm-3".to_owned()]);
    let failed = outcome.failed.expect("first failure reported");
    assert_eq!(failed.item_id, "itm-missing");
}

#[test]
fn duplicate_targets_write_distinct_log_rows() {
    let s = store();
    let bank = "bank-dup";
    let s_ref = &*s as &dyn Store;
    s_ref
        .create_memory_item(&item("itm-1", bank, "duplicate me"), "actor")
        .unwrap();
    // The same item id twice in one batch: both actions apply and both log
    // rows persist — the row id carries the target index, so the second
    // insert cannot collide on the hygiene_audit_logs primary key.
    let targets = vec![target("itm-1"), target("itm-1")];
    let outcome = clean(&args(&s, &ctx(bank), &targets, CleanAction::Flag, true)).unwrap();
    assert_eq!(outcome.applied.len(), 2);
    assert!(outcome.failed.is_none());
    let rows = s.list_hygiene_logs(Some(bank), 50, 0).unwrap();
    assert_eq!(
        rows.len(),
        2,
        "duplicate targets must not collide on the log"
    );
    assert_ne!(rows[0].id, rows[1].id);
    assert!(rows.iter().all(|r| r.id.starts_with("hlg-batch-1-")));
}

#[test]
fn preflight_rejects_confirmed_keep_that_resolves_discard_without_reason() {
    let s = store();
    let bank = "bank-keepdisc";
    let s_ref = &*s as &dyn Store;
    s_ref
        .create_memory_item(&item("keep-flag", bank, "flag me"), "actor")
        .unwrap();
    s_ref
        .create_memory_item(&item("keep-disc", bank, "discard me"), "actor")
        .unwrap();
    let discard_suggested = CleanTarget {
        item_id: "keep-disc".into(),
        noise_score: Some(0.9),
        suggested_action: Some("discard".into()),
    };
    let targets = vec![target("keep-flag"), discard_suggested];
    let opctx = ctx(bank);
    let mut a = args(&s, &opctx, &targets, CleanAction::Keep, true);
    a.reason = "";
    // Preflight must reject before any mutation — neither target is touched.
    let err = clean(&a).unwrap_err();
    assert_eq!(err.code, clio_types::ErrorCode::InvalidArgument);
    assert!(!s_ref.item_is_archived(bank, "keep-flag").unwrap());
    let logs = s.list_hygiene_logs(Some(bank), 50, 0).unwrap();
    assert!(
        logs.is_empty(),
        "no log rows before or after preflight reject"
    );

    // `keep` resolving only flag/archive still passes the preflight gate.
    let flag_only = vec![target("keep-flag")];
    let flag_ctx = ctx(bank);
    let mut ok = args(&s, &flag_ctx, &flag_only, CleanAction::Keep, true);
    ok.reason = "";
    let outcome = clean(&ok).unwrap();
    assert!(outcome.failed.is_none());
    assert_eq!(outcome.applied, vec!["keep-flag".to_owned()]);
}
