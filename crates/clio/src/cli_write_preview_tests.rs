// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the dry-run preview checks and counts: the bank/scope
//! validation, the read-only `summarize` and `consolidate` counts, and the
//! no-write guarantee behind them.
//!
//! ## Owns
//! - Unit tests for `cli_write_preview`
//!
//! ## Does not own
//! - The module under test (`cli_write_preview`)
//! - The engine-level preview emission tests (see `cli_write_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::cli_args::ParsedArgs;
use crate::cli_error::CliError;
use crate::cli_read_test_support::{extract_state, seed_item, state, tokens};
use crate::cli_write_preview::{preview_counts, validate_preview};
use clio_types::{AmError, ErrorCode};
use serde_json::json;

fn parsed(verb: &str, args: &[&str]) -> ParsedArgs {
    let spec = crate::cli_write::spec_for(verb).expect("spec");
    crate::cli_args::parse(&spec, tokens(args)).expect("parse")
}

/// The bank-scope summarize counts the work the real call would do: every
/// active item holding a snapshot is a gist to regenerate.
#[test]
fn summarize_bank_counts_the_gists_the_scope_holds() {
    let s = extract_state();
    seed_item(&s, "demo", "itm-a", "prefers aisle seats");
    seed_item(&s, "demo", "itm-b", "prefers terse answers");
    let parsed = parsed("summarize", &["--scope", "bank", "--bank", "demo"]);
    let bank = validate_preview(&s, "summarize", &parsed).expect("validated");
    let counts = preview_counts(&s, "summarize", &parsed, &bank).expect("counts");
    assert_eq!(counts["gists"], json!(2), "{counts}");
    assert_eq!(counts["skipped"], json!(0), "{counts}");
}

/// The item-scope summarize counts the one item it would regenerate.
#[test]
fn summarize_item_counts_the_named_item() {
    let s = extract_state();
    seed_item(&s, "demo", "itm-a", "prefers aisle seats");
    let parsed = parsed("summarize", &["--scope", "itm-a", "--bank", "demo"]);
    let bank = validate_preview(&s, "summarize", &parsed).expect("validated");
    let counts = preview_counts(&s, "summarize", &parsed, &bank).expect("counts");
    assert_eq!(counts["gists"], json!(1), "{counts}");
    assert_eq!(counts["skipped"], json!(0), "{counts}");
}

/// With no extractor configured the dry run fails exactly as the real call
/// does: the same `not_implemented` code, message, and exit domain, before it
/// counts anything, so it can never preview success where the real run fails.
#[test]
fn summarize_without_an_extractor_fails_like_the_real_call() {
    let s = state();
    seed_item(&s, "demo", "itm-a", "prefers aisle seats");
    let parsed = parsed("summarize", &["--scope", "bank", "--bank", "demo"]);
    let bank = validate_preview(&s, "summarize", &parsed).expect("bank is known");
    let error = preview_counts(&s, "summarize", &parsed, &bank).expect_err("gate fails");
    assert_eq!(
        error,
        CliError::from_am_error(&AmError::new(
            ErrorCode::NotImplemented,
            "extraction unavailable: no extractor configured (set extract.url to a local \
             extraction sidecar, or extract.provider=openai with a credential); no gist was \
             regenerated and no text was fabricated",
        )),
        "{error:?}"
    );
}

/// The consolidate counts the wave's pending work: the dirty ancestors it
/// would refresh and the consolidated units it would reconcile.
#[test]
fn consolidate_counts_the_pending_work() {
    let s = state();
    seed_item(&s, "demo", "itm-a", "prefers aisle seats");
    let parsed = parsed("consolidate", &["--bank", "demo"]);
    let bank = validate_preview(&s, "consolidate", &parsed).expect("validated");
    let counts = preview_counts(&s, "consolidate", &parsed, &bank).expect("counts");
    assert_eq!(counts["dirty_ancestors"], json!(1), "{counts}");
    assert_eq!(counts["consolidated_units"], json!(0), "{counts}");
}

/// `--force` refreshes every internal node, so the count reads the tree the
/// wave would dirty instead of the current dirty set.
#[test]
fn consolidate_force_counts_the_internal_nodes() {
    let s = state();
    seed_item(&s, "demo", "itm-a", "prefers aisle seats");
    let parsed = parsed("consolidate", &["--bank", "demo", "--force"]);
    let bank = validate_preview(&s, "consolidate", &parsed).expect("validated");
    let counts = preview_counts(&s, "consolidate", &parsed, &bank).expect("counts");
    assert_eq!(counts["dirty_ancestors"], json!(1), "{counts}");
}

/// An unknown bank is refused exactly as the real call would refuse it, so a
/// dry run never previews a target the store does not hold.
#[test]
fn validate_refuses_an_unknown_bank() {
    let s = state();
    let parsed = parsed("consolidate", &["--bank", "nope"]);
    let error = validate_preview(&s, "consolidate", &parsed).expect_err("refused");
    assert_eq!(
        error,
        CliError::operational("not_found", "--bank `nope` not found"),
        "{error:?}"
    );
}

/// An item scope that names no stored item is refused; a stored item passes.
#[test]
fn validate_refuses_an_unknown_item_scope() {
    let s = state();
    seed_item(&s, "demo", "itm-a", "prefers aisle seats");
    let missing = parsed("summarize", &["--scope", "missing", "--bank", "demo"]);
    let error = validate_preview(&s, "summarize", &missing).expect_err("refused");
    assert_eq!(
        error,
        CliError::operational("item_not_found", "item `missing` not found in bank"),
        "{error:?}"
    );
    let found = parsed("summarize", &["--scope", "itm-a", "--bank", "demo"]);
    assert!(
        validate_preview(&s, "summarize", &found).is_ok(),
        "{found:?}"
    );
}

/// The counts only read: the store state they measure is the same before and
/// after the preview reads, so a dry run never changes what it previews.
#[test]
fn counts_leave_the_store_unchanged() {
    let s = extract_state();
    seed_item(&s, "demo", "itm-a", "prefers aisle seats");
    seed_item(&s, "demo", "itm-b", "prefers terse answers");
    let before = (
        s.maint.try_dirty_ancestor_count("demo"),
        s.store.list_distilled_units("demo").expect("units").len(),
        crate::cli_write_test_support::bank_count(&s, "demo"),
    );
    for verb_args in [
        ("summarize", vec!["--scope", "bank", "--bank", "demo"]),
        ("consolidate", vec!["--bank", "demo"]),
        ("consolidate", vec!["--bank", "demo", "--force"]),
    ] {
        let parsed = parsed(verb_args.0, &verb_args.1);
        let bank = validate_preview(&s, verb_args.0, &parsed).expect("validated");
        preview_counts(&s, verb_args.0, &parsed, &bank).expect("counts");
    }
    let after = (
        s.maint.try_dirty_ancestor_count("demo"),
        s.store.list_distilled_units("demo").expect("units").len(),
        crate::cli_write_test_support::bank_count(&s, "demo"),
    );
    assert_eq!(before, after, "the preview reads must change nothing");
}
