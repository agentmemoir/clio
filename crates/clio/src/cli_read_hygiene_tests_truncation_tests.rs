// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the audit text view's truncated-page note: the printed
//! row count, the truncation word, and the next-page flag.
//!
//! ## Owns
//! - Text-view truncation tests for `hygiene audit`
//!
//! ## Does not own
//! - The implementation under test (`cli_read_hygiene`)
//! - The JSON page's `truncated` marker tests (`cli_read_hygiene_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::render_audit;
use crate::cli_read_test_support::{run_state, seed_item, state};
use serde_json::json;
use std::process::ExitCode;

#[test]
fn render_audit_names_a_truncated_page_and_a_full_page_stays_plain() {
    assert_eq!(
        render_audit(&json!({
            "bank": "demo", "scanned": 3, "total_candidates": 3,
            "truncated": true,
            "candidates": [
                {"item_id": "itm-1", "noise_score": 0.9},
                {"item_id": "itm-2", "noise_score": 0.8}
            ]
        })),
        "hygiene audit (bank demo): scanned 3 active items, 3 candidates, \
         printed 2 rows (truncated: use --offset for the next page)\n  0.90  -  itm-1  \n  0.80  -  itm-2  "
    );
    // A full page carries no truncation note: the header stays byte-identical
    // when every candidate is on the page.
    assert_eq!(
        render_audit(&json!({
            "bank": "demo", "scanned": 1, "total_candidates": 1,
            "truncated": false,
            "candidates": [{"item_id": "itm-1", "noise_score": 0.9}]
        })),
        "hygiene audit (bank demo): scanned 1 active item, 1 candidate\n  0.90  -  itm-1  "
    );
}

#[test]
fn audit_text_reports_a_short_page_and_keeps_a_full_page_plain() {
    let s = state();
    seed_item(&s, "demo", "itm-t1", "stale duplicated note one");
    seed_item(&s, "demo", "itm-t2", "stale duplicated note two");
    let (code, out, err) = run_state(
        "hygiene audit",
        &["--bank", "demo", "--min-score", "0", "--limit", "1"],
        &s,
        true,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    assert!(
        out.contains("printed 1 row (truncated: use --offset for the next page)"),
        "{out}"
    );

    let (code, full, err) = run_state(
        "hygiene audit",
        &["--bank", "demo", "--min-score", "0"],
        &s,
        true,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{full}{err}");
    assert!(!full.contains("printed"), "{full}");
    assert!(!full.contains("truncated"), "{full}");
}

/// A short page that is not truncated still prints fewer rows than the total,
/// so the header names the row count without the truncation word the JSON
/// form does not carry either.
#[test]
fn audit_text_names_printed_rows_when_a_short_page_is_not_truncated() {
    let s = state();
    for n in 0..5 {
        seed_item(
            &s,
            "demo",
            &format!("itm-short-{n}"),
            &format!("stale duplicated note {n}"),
        );
    }
    let (code, out, err) = run_state(
        "hygiene audit",
        &["--bank", "demo", "--min-score", "0", "--offset", "3"],
        &s,
        true,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    assert!(out.contains("printed 2 rows"), "{out}");
    assert!(!out.contains("truncated"), "{out}");
}

/// An offset past the end prints no rows, and the header still names that
/// count without calling the empty page truncated.
#[test]
fn audit_text_names_printed_zero_rows_when_the_offset_is_past_the_end() {
    let s = state();
    seed_item(&s, "demo", "itm-past-1", "stale duplicated note one");
    seed_item(&s, "demo", "itm-past-2", "stale duplicated note two");
    let (code, out, err) = run_state(
        "hygiene audit",
        &["--bank", "demo", "--min-score", "0", "--offset", "9"],
        &s,
        true,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    assert!(out.contains("printed 0 rows"), "{out}");
    assert!(!out.contains("truncated"), "{out}");
}

/// A truncated page requested with a non-zero offset keeps the full truncated
/// wording, including the next-page flag.
#[test]
fn audit_text_keeps_the_full_truncated_wording_for_an_offset_page() {
    let s = state();
    for n in 0..3 {
        seed_item(
            &s,
            "demo",
            &format!("itm-off-{n}"),
            &format!("stale duplicated note {n}"),
        );
    }
    let (code, out, err) = run_state(
        "hygiene audit",
        &[
            "--bank",
            "demo",
            "--min-score",
            "0",
            "--limit",
            "1",
            "--offset",
            "1",
        ],
        &s,
        true,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    assert!(
        out.contains("printed 1 row (truncated: use --offset for the next page)"),
        "{out}"
    );
}

/// The clamp note and the truncated note stack in the same header line, clamp
/// first, then the printed-row note.
#[test]
fn audit_text_stacks_the_clamp_note_and_the_truncated_note_in_one_header_line() {
    let text = render_audit(&json!({
        "bank": "demo", "scanned": 3, "total_candidates": 3,
        "truncated": true, "requested_limit": 500,
        "candidates": [
            {"item_id": "itm-1", "noise_score": 0.9},
            {"item_id": "itm-2", "noise_score": 0.8}
        ]
    }));
    let header = text.lines().next().expect("header line");
    assert!(header.contains("(limit 200, lowered from 500)"), "{header}");
    assert!(
        header.contains("printed 2 rows (truncated: use --offset for the next page)"),
        "{header}"
    );
    let clamp = header.find("(limit 200").expect("clamp note");
    let printed = header.find("printed 2 rows").expect("truncated note");
    assert!(clamp < printed, "{header}");
}
