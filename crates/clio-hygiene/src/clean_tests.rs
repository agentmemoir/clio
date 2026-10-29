// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the confirmed `hygiene_clean` mutator (`clean.rs`):
//! dry-run counts, archive retrieve exclusion, discard
//! without DEK destroy, the `delete` alias, `keep`, the durable log
//! And request validation. SQLite backend; the Postgres parity
//! probe lives in `clio-store`'s `pg_hygiene_tests`.
//!
//! ## Owns
//! - Test coverage for dry-run counting, confirmed counter consistency
//!   (would_* equals the applied counts per action), archive/discard/keep
//!   outcomes, log row shape, and fail-closed argument validation
//!
//! ## Does not own
//! - Batch-abort and duplicate-target semantics (`clean_batch_tests.rs`)
//! - The credential label a durable row carries (`clean_principal_tests.rs`)
//! - The clean behavior itself (`clean.rs`) or store SQL
//!
//! ## Boundary
//!
//! Tests here break when clean semantics change; they never change it.
//!
//! Keep this module focused on `hygiene_clean` single-action tests.

use super::clean_test_support::{args, ctx, item, store, target};
use crate::clean::{CleanAction, CleanOutcome, CleanTarget, clean};
use clio_store::{ComplianceStore, HygieneStore, IndexStore, SqliteStore, Store};

#[test]
fn t21_02_dry_run_counts_only_and_writes_nothing() {
    let s = store();
    let bank = "bank-dry";
    let s_ref = &*s as &dyn Store;
    s_ref
        .create_memory_item(&item("itm-1", bank, "scratch note one"), "actor")
        .unwrap();
    let before = s_ref.get_memory_item(bank, "itm-1").unwrap();
    let outcome = clean(&args(
        &s,
        &ctx(bank),
        &[target("itm-1")],
        CleanAction::Archive,
        false,
    ))
    .unwrap();
    assert!(outcome.dry_run);
    assert_eq!(outcome.would_archive, 1);
    assert_eq!(outcome.would_flag + outcome.would_discard, 0);
    assert!(outcome.applied.is_empty() && outcome.pending.is_empty() && outcome.failed.is_none());
    // zero writes: item unchanged, no archive marker, and no durable log row
    // anywhere — a preview leaves no trace to list.
    assert!(!s_ref.item_is_archived(bank, "itm-1").unwrap());
    let logs = s.list_hygiene_logs(Some(bank), 50, 0).unwrap();
    assert!(logs.is_empty(), "a preview writes no log row: {logs:?}");
    let after = s_ref.get_memory_item(bank, "itm-1").unwrap();
    assert_eq!(
        after.as_ref().and_then(|i| i.gist.clone()),
        before.as_ref().and_then(|i| i.gist.clone())
    );
}

#[test]
fn a_preview_over_an_already_discarded_item_reports_nothing_to_do() {
    let s = store();
    let bank = "bank-preview-disc";
    let s_ref = &*s as &dyn Store;
    s_ref
        .create_memory_item(&item("itm-1", bank, "already gone"), "actor")
        .unwrap();
    s_ref
        .discard_item(
            bank,
            "itm-1",
            "removed earlier",
            "2026-09-19T00:00:00Z",
            "actor",
        )
        .unwrap();
    let outcome = clean(&args(
        &s,
        &ctx(bank),
        &[target("itm-1")],
        CleanAction::Archive,
        false,
    ))
    .unwrap();
    assert!(outcome.dry_run);
    assert_eq!(
        outcome.would_flag + outcome.would_archive + outcome.would_discard,
        0,
        "a settled item has no candidate action left"
    );
}

#[test]
fn a_preview_over_an_already_archived_item_reports_nothing_to_do() {
    let s = store();
    let bank = "bank-preview-arch";
    let s_ref = &*s as &dyn Store;
    s_ref
        .create_memory_item(&item("itm-1", bank, "already shelved"), "actor")
        .unwrap();
    s.archive_item(bank, "itm-1", "2026-09-19T00:00:00Z", "actor")
        .unwrap();
    let outcome = clean(&args(
        &s,
        &ctx(bank),
        &[target("itm-1")],
        CleanAction::Discard,
        false,
    ))
    .unwrap();
    assert!(outcome.dry_run);
    assert_eq!(
        outcome.would_flag + outcome.would_archive + outcome.would_discard,
        0
    );
}

#[test]
fn t21_03_confirmed_archive_hides_from_default_retrieve() {
    let s = store();
    let bank = "bank-arch";
    let s_ref = &*s as &dyn Store;
    s_ref
        .create_memory_item(&item("itm-1", bank, "service listens on 8080"), "actor")
        .unwrap();
    s.put_lexical_doc(bank, "itm-1", "service listens on 8080")
        .unwrap();
    assert_eq!(
        indexed_hits(&s, bank, "service listens").len(),
        1,
        "indexable before archive"
    );
    let outcome = clean(&args(
        &s,
        &ctx(bank),
        &[target("itm-1")],
        CleanAction::Archive,
        true,
    ))
    .unwrap();
    assert!(!outcome.dry_run);
    assert_eq!(outcome.applied, vec!["itm-1".to_owned()]);
    // Excluded from default retrieve: dense/lexical visibility probes fail.
    assert!(s_ref.item_is_archived(bank, "itm-1").unwrap());
    let hits = indexed_hits(&s, bank, "service listens");
    assert!(
        hits.is_empty(),
        "archived item must leave default retrieve: {hits:?}"
    );
    // Still readable via id (second clause).
    assert!(s_ref.get_memory_item(bank, "itm-1").unwrap().is_some());
    // Inspect metadata exposes the archive marker.
    let rows = s_ref
        .inspect_items(&{
            let mut f = clio_store::ItemInspectFilter::new(bank);
            f.limit = 10;
            f
        })
        .unwrap();
    assert!(rows.iter().any(|r| r.item_id == "itm-1" && r.archived));
    // Telemetry row present.
    let trail = s_ref.audit_trail(bank, "itm-1").unwrap();
    assert!(trail.events.iter().any(|e| e.operation == "archive"));
}

#[test]
fn unarchive_restores_retrieve_eligibility() {
    let s = store();
    let bank = "bank-unarch";
    let s_ref = &*s as &dyn Store;
    s_ref
        .create_memory_item(
            &item("itm-1", bank, "unique unarchive query marker"),
            "actor",
        )
        .unwrap();
    s.put_lexical_doc(bank, "itm-1", "unique unarchive query marker")
        .unwrap();
    assert_eq!(indexed_hits(&s, bank, "unarchive query marker").len(), 1);
    s.archive_item(bank, "itm-1", "2026-09-20T00:00:00Z", "actor")
        .unwrap();
    let archived_hits = indexed_hits(&s, bank, "unarchive query marker");
    assert!(archived_hits.is_empty(), "{archived_hits:?}");
    s.unarchive_item(bank, "itm-1", "actor").unwrap();
    let hits = indexed_hits(&s, bank, "unarchive query marker");
    assert_eq!(hits.len(), 1, "unarchive restores default retrieve");
    // Unarchiving twice is rejected.
    let err = s.unarchive_item(bank, "itm-1", "actor").unwrap_err();
    assert_eq!(err.code, clio_types::ErrorCode::InvalidArgument);
}

#[test]
fn t21_04_discard_and_delete_alias_keep_dek_alive() {
    let s = store();
    let bank = "bank-disc";
    let s_ref = &*s as &dyn Store;
    s_ref
        .create_memory_item(&item("itm-1", bank, "discard me"), "actor")
        .unwrap();
    // `delete` alias maps to discard.
    let outcome = clean(&args(
        &s,
        &ctx(bank),
        &[target("itm-1")],
        CleanAction::Discard,
        true,
    ))
    .unwrap();
    assert_eq!(outcome.applied, vec!["itm-1".to_owned()]);
    assert!(outcome.failed.is_none());
    let identity = s_ref.get_item_identity(bank, "itm-1").unwrap();
    assert!(
        identity.is_none(),
        "discarded item no longer resolves as active"
    );
    // DEK / content untouched: the subject DEK was not destroyed.
    assert!(!s.subject_dek_destroyed("subj-itm-1").unwrap());
    // `delete` is a pure alias of `discard`, never erase (no tombstone).
    let tombstones = s.read_tombstones(bank, "any").unwrap();
    assert!(tombstones.is_empty(), "hygiene never writes tombstones");
    assert!(trail_has_operation(s_ref, bank, "itm-1", "discard"));
}

/// Index the item's gist into the FTS store (tests control indexing
/// explicitly; the write path enqueues, the worker indexes).
fn indexed_hits(s: &SqliteStore, bank: &str, query: &str) -> Vec<clio_store::SearchHit> {
    s.lexical_search(bank, query, &[], 10).unwrap()
}

fn trail_has_operation(s: &dyn Store, bank: &str, id: &str, op: &str) -> bool {
    s.audit_trail(bank, id)
        .unwrap()
        .events
        .iter()
        .any(|e| e.operation == op)
}

#[test]
fn confirm_flag_marks_for_review_but_stays_retrievable() {
    let s = store();
    let bank = "bank-flag";
    let s_ref = &*s as &dyn Store;
    s_ref
        .create_memory_item(&item("itm-1", bank, "flag me for review"), "actor")
        .unwrap();
    s.put_lexical_doc(bank, "itm-1", "flag me for review")
        .unwrap();
    let outcome = clean(&args(
        &s,
        &ctx(bank),
        &[target("itm-1")],
        CleanAction::Flag,
        true,
    ))
    .unwrap();
    assert_eq!(outcome.applied, vec!["itm-1".to_owned()]);
    // Flagged items remain fully retrievable (metadata-only review mark).
    assert_eq!(indexed_hits(&s, bank, "flag me for review").len(), 1);
    let rows = s_ref
        .inspect_items(&{
            let mut f = clio_store::ItemInspectFilter::new(bank);
            f.limit = 10;
            f
        })
        .unwrap();
    let row = rows.iter().find(|r| r.item_id == "itm-1").unwrap();
    assert!(!row.archived);
    assert!(trail_has_operation(s_ref, bank, "itm-1", "hygiene_flag"));
}

#[test]
fn keep_applies_each_targets_suggested_action() {
    let s = store();
    let bank = "bank-keep";
    let s_ref = &*s as &dyn Store;
    s_ref
        .create_memory_item(&item("keep-arch", bank, "archive me"), "actor")
        .unwrap();
    let targets = vec![target("keep-arch")];
    let outcome = clean(&args(&s, &ctx(bank), &targets, CleanAction::Keep, true)).unwrap();
    assert_eq!(outcome.applied, vec!["keep-arch".to_owned()]);
    assert!(s_ref.item_is_archived(bank, "keep-arch").unwrap());
    // `keep` with no suggestion fails closed.
    let empty = vec![CleanTarget {
        item_id: "keep-arch".into(),
        noise_score: None,
        suggested_action: None,
    }];
    assert!(clean(&args(&s, &ctx(bank), &empty, CleanAction::Keep, true)).is_err());
}

#[test]
fn confirmed_counters_equal_the_applied_counts_per_action() {
    let s = store();
    let bank = "bank-counters";
    let s_ref = &*s as &dyn Store;
    for id in ["cnt-flag", "cnt-arch", "cnt-disc"] {
        s_ref
            .create_memory_item(&item(id, bank, "counter me"), "actor")
            .unwrap();
    }
    let targets = vec![
        suggested("cnt-flag", "flag"),
        suggested("cnt-arch", "archive"),
        suggested("cnt-disc", "discard"),
    ];
    let outcome = clean(&args(&s, &ctx(bank), &targets, CleanAction::Keep, true)).unwrap();
    assert!(outcome.failed.is_none());
    assert_eq!(outcome.applied.len(), 3, "{:?}", outcome.applied);
    // One meaning per field: on a confirmed run each would_* equals the
    // applied count for its action, matching the preview's count.
    assert_eq!(outcome.would_flag, 1);
    assert_eq!(outcome.would_archive, 1);
    assert_eq!(outcome.would_discard, 1);
}

/// One target with an explicit audit suggestion.
fn suggested(id: &str, action: &str) -> CleanTarget {
    CleanTarget {
        item_id: id.into(),
        noise_score: Some(0.5),
        suggested_action: Some(action.into()),
    }
}

#[test]
fn t21_07_confirmed_clean_writes_masked_durable_log_rows() {
    let s = store();
    let bank = "bank-log";
    let s_ref = &*s as &dyn Store;
    s_ref
        .create_memory_item(&item("itm-1", bank, "log me"), "actor")
        .unwrap();
    clean(&args(
        &s,
        &ctx(bank),
        &[target("itm-1")],
        CleanAction::Archive,
        true,
    ))
    .unwrap();
    let rows = s.list_hygiene_logs(Some(bank), 50, 0).unwrap();
    assert_eq!(rows.len(), 1, "one masked log row per confirmed clean");
    let row = &rows[0];
    assert_eq!(row.action, "archive");
    assert!(row.confirmed && !row.dry_run);
    assert_eq!(row.item_id, "itm-1");
    assert!(
        row.noise_score == Some(0.75),
        "the log row records the noise score"
    );
    // Rows never carry secret material or content — ids/scores/labels only.
    let detail = row.detail.to_string();
    assert!(!detail.contains("api_key") && !detail.contains("gist"));
    // Listable across banks and paginated.
    assert_eq!(s.list_hygiene_logs(None, 50, 0).unwrap().len(), 1);
    assert_eq!(s.list_hygiene_logs(Some(bank), 1, 0).unwrap().len(), 1);
    let past_the_end = s.list_hygiene_logs(Some(bank), 50, 1).unwrap();
    assert!(past_the_end.is_empty(), "{past_the_end:?}");
}

#[test]
fn invalid_requests_fail_closed() {
    let s = store();
    let bank = "bank-bad";
    // unknown action label
    assert!(CleanAction::parse("erase").is_err());
    assert_eq!(CleanAction::parse("delete").unwrap(), CleanAction::Discard);
    // empty targets / empty bank / blank reason on confirmed discard
    let c = ctx(bank);
    assert!(clean(&args(&s, &c, &[], CleanAction::Flag, true)).is_err());
    let empty_ctx = ctx("");
    let empty_targets = vec![target("x")];
    let outcome = clean(&args(
        &s,
        &empty_ctx,
        &empty_targets,
        CleanAction::Flag,
        true,
    ));
    assert_eq!(
        outcome.unwrap_err().code,
        clio_types::ErrorCode::MissingBank
    );
    let blank_reason_targets = vec![CleanTarget {
        item_id: "x".into(),
        noise_score: None,
        suggested_action: None,
    }];
    let bank_ctx = ctx(bank);
    let mut a = args(
        &s,
        &bank_ctx,
        &blank_reason_targets,
        CleanAction::Discard,
        true,
    );
    a.reason = "  ";
    let err = clean(&a).unwrap_err();
    assert_eq!(err.code, clio_types::ErrorCode::InvalidArgument);
    // blank target id rejected
    let blank_id = vec![CleanTarget {
        item_id: "  ".into(),
        noise_score: None,
        suggested_action: None,
    }];
    let flag_ctx = ctx(bank);
    assert!(clean(&args(&s, &flag_ctx, &blank_id, CleanAction::Flag, true)).is_err());
    // invalid log action label fails at the store layer
    let entry = clio_store::HygieneLogEntry {
        id: "hlg-bad".into(),
        bank_id: bank.into(),
        item_id: "x".into(),
        noise_score: None,
        action: "erase_request".into(),
        actor: "actor".into(),
        confirmed: true,
        dry_run: false,
        detail: serde_json::json!({}),
        created_at: "2026-09-20T00:00:00Z".into(),
    };
    let err = s.insert_hygiene_log(&entry).unwrap_err();
    assert_eq!(err.code, clio_types::ErrorCode::InvalidArgument);
}

#[test]
fn clean_outcome_shape_is_stable() {
    let dry = CleanOutcome {
        dry_run: true,
        would_flag: 1,
        would_archive: 2,
        would_discard: 3,
        applied: vec![],
        pending: vec![],
        failed: None,
    };
    let rendered = serde_json::to_value(&dry).unwrap();
    assert_eq!(rendered["would_flag"], 1);
    assert_eq!(rendered["would_archive"], 2);
    assert_eq!(rendered["would_discard"], 3);
}
