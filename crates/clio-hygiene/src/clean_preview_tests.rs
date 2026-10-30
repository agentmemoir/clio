// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `hygiene_clean` preview branch: dry-run counting
//! with zero writes, the settled-target skip (already discarded or
//! archived), and preview/confirmed agreement over settled targets.
//!
//! ## Owns
//! - Test coverage for the preview's counters, its `skipped` list, and the
//!   preview→confirmed parity for settled targets
//!
//! ## Does not own
//! - The confirmed mutation outcomes (`clean_tests.rs`)
//! - The per-target `apply_suggested` resolution and skip behaviour
//!   (`clean_apply_suggested_tests.rs`)
//! - The clean behavior itself (`clean.rs`, `clean_preview.rs`) or store SQL
//!
//! ## Boundary
//!
//! Tests here break when the preview semantics change; they never change it.
//!
//! Keep this module focused on `hygiene_clean` preview tests.

use super::clean_test_support::{args, ctx, item, store, target};
use crate::clean::{CleanAction, clean};
use clio_store::{HygieneStore, Store};

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
fn preview_and_confirm_agree_over_one_discarded_and_one_archived_target() {
    let s = store();
    let bank = "bank-parity-settled";
    let s_ref = &*s as &dyn Store;
    for id in ["itm-disc", "itm-arch"] {
        s_ref
            .create_memory_item(&item(id, bank, "already settled"), "actor")
            .unwrap();
    }
    s_ref
        .discard_item(
            bank,
            "itm-disc",
            "removed earlier",
            "2026-09-19T00:00:00Z",
            "actor",
        )
        .unwrap();
    s.archive_item(bank, "itm-arch", "2026-09-19T00:00:00Z", "actor")
        .unwrap();
    let targets = vec![target("itm-disc"), target("itm-arch")];
    // The preview: both settled targets are skipped, none counted.
    let preview = clean(&args(&s, &ctx(bank), &targets, CleanAction::Flag, false)).unwrap();
    assert!(preview.dry_run);
    assert_eq!(
        preview.would_flag + preview.would_archive + preview.would_discard,
        0,
        "{preview:?}"
    );
    assert_eq!(
        preview.skipped,
        vec!["itm-disc".to_owned(), "itm-arch".to_owned()],
        "{preview:?}"
    );
    // The confirmed run over the same request: the same skipped set, the
    // same zero counters, no failure, no half-applied state.
    let confirmed = clean(&args(&s, &ctx(bank), &targets, CleanAction::Flag, true)).unwrap();
    assert!(confirmed.failed.is_none(), "{:?}", confirmed.failed);
    assert!(confirmed.pending.is_empty(), "{confirmed:?}");
    assert!(confirmed.applied.is_empty(), "{confirmed:?}");
    assert_eq!(
        confirmed.would_flag + confirmed.would_archive + confirmed.would_discard,
        0,
        "{confirmed:?}"
    );
    assert_eq!(confirmed.skipped, preview.skipped, "{confirmed:?}");
    // Neither mode wrote: no archive marker change, no durable log row.
    let rows = s.list_hygiene_logs(Some(bank), 50, 0).unwrap();
    assert!(rows.is_empty(), "{rows:?}");
}
