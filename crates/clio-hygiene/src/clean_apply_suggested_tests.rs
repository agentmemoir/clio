// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `apply_suggested` action of the confirmed
//! `hygiene_clean` mutator (`clean.rs`): per-target suggestion resolution and
//! the skip-not-abort behaviour when a target's own suggestion is `keep`.
//!
//! ## Owns
//! - Test coverage for per-target suggestion application, the reported
//!   `skipped` list (confirmed and preview), and the preview's reported skip
//!
//! ## Does not own
//! - The single-action outcomes (`clean_tests.rs`)
//! - Batch abort and duplicate-target semantics (`clean_batch_tests.rs`)
//!
//! ## Boundary
//!
//! Tests here break when the `apply_suggested` semantics change; they never
//! change it.
//!
//! Keep this module focused on `apply_suggested` resolution tests.

use super::clean_test_support::{args, ctx, item, store};
use crate::clean::{CleanAction, CleanTarget, clean};
use clio_store::{HygieneStore, Store};

/// One target with an explicit audit suggestion.
fn suggested(id: &str, action: &str) -> CleanTarget {
    CleanTarget {
        item_id: id.into(),
        noise_score: Some(0.5),
        suggested_action: Some(action.into()),
    }
}

/// One target whose audit suggestion is `keep` (nothing to apply).
fn keep_suggested(id: &str) -> CleanTarget {
    suggested(id, "keep")
}

#[test]
fn apply_suggested_parses_and_the_old_keep_spelling_is_refused() {
    assert_eq!(
        CleanAction::parse("apply_suggested").unwrap(),
        CleanAction::ApplySuggested
    );
    let err = CleanAction::parse("keep").unwrap_err();
    assert_eq!(err.code, clio_types::ErrorCode::InvalidArgument);
    assert!(
        err.message
            .contains("flag|archive|discard|delete|apply_suggested"),
        "{err}"
    );
}

#[test]
fn apply_suggested_applies_each_targets_own_suggestion() {
    let s = store();
    let bank = "bank-apply-each";
    let s_ref = &*s as &dyn Store;
    for id in ["as-flag", "as-arch", "as-disc"] {
        s_ref
            .create_memory_item(&item(id, bank, "apply my suggestion"), "actor")
            .unwrap();
    }
    let targets = vec![
        suggested("as-flag", "flag"),
        suggested("as-arch", "archive"),
        suggested("as-disc", "discard"),
    ];
    let outcome = clean(&args(
        &s,
        &ctx(bank),
        &targets,
        CleanAction::ApplySuggested,
        true,
    ))
    .unwrap();
    assert!(outcome.failed.is_none() && outcome.pending.is_empty());
    assert!(outcome.skipped.is_empty(), "{:?}", outcome.skipped);
    assert_eq!(outcome.applied.len(), 3, "{:?}", outcome.applied);
    assert_eq!(outcome.would_flag, 1);
    assert_eq!(outcome.would_archive, 1);
    assert_eq!(outcome.would_discard, 1);
    assert!(s_ref.item_is_archived(bank, "as-arch").unwrap());
    let trail = s_ref.audit_trail(bank, "as-flag").unwrap();
    assert!(trail.events.iter().any(|e| e.operation == "hygiene_flag"));
    assert!(
        s_ref.get_item_identity(bank, "as-disc").unwrap().is_none(),
        "the discard suggestion applied"
    );
}

#[test]
fn a_keep_suggestion_is_skipped_and_the_rest_of_the_batch_still_applies() {
    let s = store();
    let bank = "bank-skip";
    let s_ref = &*s as &dyn Store;
    for id in ["sk-keep", "sk-arch", "sk-flag"] {
        s_ref
            .create_memory_item(&item(id, bank, "skip me or clean me"), "actor")
            .unwrap();
    }
    // The `keep`-suggestion target comes first: the batch must continue
    // past it, not abort.
    let targets = vec![
        keep_suggested("sk-keep"),
        suggested("sk-arch", "archive"),
        suggested("sk-flag", "flag"),
    ];
    let outcome = clean(&args(
        &s,
        &ctx(bank),
        &targets,
        CleanAction::ApplySuggested,
        true,
    ))
    .unwrap();
    assert!(outcome.failed.is_none(), "{:?}", outcome.failed);
    assert!(outcome.pending.is_empty(), "{:?}", outcome.pending);
    assert_eq!(outcome.skipped, vec!["sk-keep".to_owned()]);
    assert_eq!(
        outcome.applied,
        vec!["sk-arch".to_owned(), "sk-flag".to_owned()]
    );
    // The other candidates in the same batch applied for real.
    assert!(s_ref.item_is_archived(bank, "sk-arch").unwrap());
    let trail = s_ref.audit_trail(bank, "sk-flag").unwrap();
    assert!(trail.events.iter().any(|e| e.operation == "hygiene_flag"));
    // The skipped target was not touched: no archive marker, no log row.
    assert!(!s_ref.item_is_archived(bank, "sk-keep").unwrap());
    let rows = s.list_hygiene_logs(Some(bank), 50, 0).unwrap();
    assert_eq!(rows.len(), 2, "one log row per applied target: {rows:?}");
    assert!(rows.iter().all(|r| r.item_id != "sk-keep"));
}

#[test]
fn a_preview_skips_a_keep_suggestion_without_failing() {
    let s = store();
    let bank = "bank-skip-preview";
    let s_ref = &*s as &dyn Store;
    for id in ["pv-keep", "pv-arch"] {
        s_ref
            .create_memory_item(&item(id, bank, "preview me"), "actor")
            .unwrap();
    }
    let targets = vec![keep_suggested("pv-keep"), suggested("pv-arch", "archive")];
    let outcome = clean(&args(
        &s,
        &ctx(bank),
        &targets,
        CleanAction::ApplySuggested,
        false,
    ))
    .unwrap();
    assert!(outcome.dry_run);
    assert_eq!(outcome.would_archive, 1);
    assert_eq!(outcome.would_flag + outcome.would_discard, 0);
    // The preview discloses the skip in the same field a confirmed run
    // uses, so the two modes agree for the same request.
    assert_eq!(outcome.skipped, vec!["pv-keep".to_owned()]);
    // Zero writes, as for any preview.
    assert!(!s_ref.item_is_archived(bank, "pv-arch").unwrap());
    let logs = s.list_hygiene_logs(Some(bank), 50, 0).unwrap();
    assert!(logs.is_empty(), "{logs:?}");
}

#[test]
fn an_unmapped_suggestion_still_fails_closed() {
    let s = store();
    let bank = "bank-unmapped";
    let targets = vec![suggested("unmapped-1", "erase")];
    let err = clean(&args(
        &s,
        &ctx(bank),
        &targets,
        CleanAction::ApplySuggested,
        true,
    ))
    .unwrap_err();
    assert_eq!(err.code, clio_types::ErrorCode::InvalidArgument);
    assert!(
        err.message.contains("no applicable suggested_action"),
        "{err}"
    );
}

#[test]
fn a_missing_suggestion_still_fails_closed() {
    let s = store();
    let bank = "bank-missing";
    // A bare target with no audit suggestion (an `item_ids` spelling) has no
    // applicable action other than `keep`; it fails closed.
    let bare = vec![CleanTarget {
        item_id: "bare-1".into(),
        noise_score: None,
        suggested_action: None,
    }];
    let err = clean(&args(
        &s,
        &ctx(bank),
        &bare,
        CleanAction::ApplySuggested,
        true,
    ))
    .unwrap_err();
    assert_eq!(err.code, clio_types::ErrorCode::InvalidArgument);
    assert!(
        err.message.contains("no applicable suggested_action"),
        "{err}"
    );
}
