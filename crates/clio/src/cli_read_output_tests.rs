// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests read-command output selection and the recall text view:
//! the piped JSON default, the TTY text default, explicit `--output`/`-o`
//! overrides, empty results, and TTY error rendering. It also pins the
//! recall text view's per-stage score breakdown, the retained relative-score
//! disclaimer, and the fact that the text view never reaches JSON output.
//!
//! ## Owns
//! - Assertions over `run_state` for output modes and the recall text view
//!
//! ## Does not own
//! - The output contract itself (see `cli_output`)
//! - The breakdown's formatting details over synthetic payloads
//!   (`cli_read_render_scores_tests`)
//! - Read-command behavior tests (`cli_read_tests`) or the shared runners
//!   (`cli_read_test_support`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on output-mode selection for read commands.

use crate::cli_read_test_support::{json_of, run_state, seed_item, state};
use clio_mcp::runtime::McpState;
use serde_json::json;
use std::process::ExitCode;
use std::sync::Arc;

/// The note the recall text view must keep above the hits.
const RELATIVE_SCORE_NOTE: &str = "score is relative (rank fusion), not calibrated relevance";

/// Seed the two-item bank every recall output test reads.
fn state_with_items() -> Arc<McpState> {
    let state = state();
    seed_item(&state, "bank-a", "itm-1", "terse answers");
    seed_item(&state, "bank-a", "itm-2", "verbose replies");
    state
}

#[test]
fn recall_piped_defaults_to_json() {
    let state = state_with_items();
    let (code, out, err) = run_state(
        "recall",
        &["terse", "--bank", "bank-a", "--actor", "agent"],
        &state,
        false,
    );
    assert_eq!(code, ExitCode::from(0));
    let value = json_of(&out);
    assert_eq!(value["ok"], json!(true));
    assert_eq!(value["hits"].as_array().map(Vec::len), Some(1));
    assert!(err.is_empty(), "stderr: {err}");
}

#[test]
fn recall_tty_text_and_explicit_output_override() {
    let state = state_with_items();
    let (code, out, _) = run_state("recall", &["terse", "--bank", "bank-a"], &state, true);
    assert_eq!(code, ExitCode::from(0));
    assert!(out.contains("Results for: terse"), "{out}");
    assert!(out.contains("terse answers"), "{out}");
    assert!(out.contains("lexical match"), "{out}");
    assert!(out.contains("score 0."), "{out}");
    assert!(out.contains("id: itm-1"), "{out}");
    assert!(!out.contains('\u{1b}'), "no ANSI when piped/no_color");

    let (_, json_out, _) = run_state(
        "recall",
        &["terse", "--bank", "bank-a", "--output", "json"],
        &state,
        true,
    );
    assert_eq!(json_of(&json_out)["ok"], json!(true));
    // The breakdown and the disclaimer are text-only; JSON keeps its own shape.
    assert!(!json_out.contains(RELATIVE_SCORE_NOTE), "{json_out}");
    assert!(!json_out.contains("scores: final"), "{json_out}");

    let (_, out, _) = run_state(
        "recall",
        &["terse", "--bank", "bank-a", "--output", "text"],
        &state,
        false,
    );
    assert!(out.contains("Results for: terse"), "{out}");
    assert!(out.contains("terse answers"), "{out}");

    // The `-o` alias selects the same modes as `--output`.
    let (_, out, _) = run_state(
        "recall",
        &["terse", "--bank", "bank-a", "-o", "json"],
        &state,
        true,
    );
    assert_eq!(json_of(&out)["ok"], json!(true));

    let (_, out, _) = run_state(
        "recall",
        &["terse", "--bank", "bank-a", "-o=text"],
        &state,
        false,
    );
    assert!(out.contains("Results for: terse"), "{out}");
}

#[test]
fn recall_empty_prints_no_results() {
    let state = state_with_items();
    let (code, out, _) = run_state("recall", &["zzz-nothing", "--bank", "bank-a"], &state, true);
    assert_eq!(code, ExitCode::from(0));
    assert_eq!(out.trim(), "no results");
}

/// The text view breaks every hit down by stage under the retained
/// relative-score disclaimer, and keeps the provenance reason line.
#[test]
fn recall_text_shows_the_per_stage_breakdown() {
    let state = state_with_items();
    let (code, out, err) = run_state("recall", &["terse", "--bank", "bank-a"], &state, true);
    assert_eq!(code, ExitCode::from(0));
    assert!(err.is_empty(), "stderr: {err}");

    let lines: Vec<&str> = out.lines().collect();
    let disclaimer = lines
        .iter()
        .position(|line| *line == RELATIVE_SCORE_NOTE)
        .unwrap_or_else(|| panic!("the relative-score disclaimer is retained: {out}"));

    // The disclaimer stays above the hits, and every hit gets one breakdown.
    let breakdown: Vec<&&str> = lines
        .iter()
        .enumerate()
        .filter(|(index, line)| {
            line.trim_start().starts_with("scores: final") && *index > disclaimer
        })
        .map(|(_, line)| line)
        .collect();
    let hits = json_of(&run_state("recall", &["terse", "--bank", "bank-a"], &state, false).1);
    let expected = hits["hits"].as_array().expect("hits").len();
    assert_eq!(breakdown.len(), expected, "one breakdown per hit: {out}");

    // This deployment runs lexical-only: the dense and rerank arms never scored
    // the hit, so both show the placeholder and neither shows a zero.
    let line = breakdown.first().expect("a hit").to_string();
    assert!(line.contains("semantic -"), "{out}");
    assert!(line.contains("reranker -"), "{out}");
    assert!(line.contains("keyword "), "{out}");
    for absent in ["semantic", "reranker"] {
        assert!(
            !line.contains(&format!("{absent} 0")),
            "{absent} as 0: {out}"
        );
    }
    // The fused value appears on the headline too, and both agree.
    let headline = lines
        .iter()
        .find(|line| line.trim_start().starts_with("1."))
        .unwrap_or_else(|| panic!("first hit: {out}"));
    assert!(
        line.contains(&format!(
            "final {}",
            headline
                .rsplit_once("score ")
                .expect("headline score fragment")
                .1
        )),
        "headline and breakdown agree: {out}"
    );
    assert!(out.contains("[lexical match]"), "provenance reason: {out}");
}

#[test]
fn parse_error_on_a_tty_renders_text() {
    let state = state_with_items();
    let (code, out, err) = run_state("recall", &["--limt"], &state, true);
    assert_eq!(code, ExitCode::from(2));
    assert!(out.is_empty(), "{out:?}");
    assert!(err.starts_with("error:"), "{err}");
}
