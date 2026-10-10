// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing what one finished wizard run hands
//! back: the answers, the signal a refusal stops on, the preview, and the
//! rendered preview text.
//!
//! ## Owns
//! - Every test of `setup_preview_run`'s `Run` accessors and the two drive modes
//!
//! ## Does not own
//! - What the preview contains (see `setup_preview_tests`)
//! - The questions themselves (see the `setup_step_*_tests` suites)
//! - The harness the drives are methods of (see `setup_preview_fixtures`)
//!
//! ## Boundary
//!
//! Test-only, like the file it tests: the run result exists only so a suite can
//! read one. Every case drives the harness with its own injected streams and
//! environment, so no test reaches the machine's config or data directory.
//!
//! Keep this module focused on the run result.

use super::*;
use crate::setup_preview_fixtures::{Harness, PASTED, SERVICES};

/// Every question answered, then whatever the test types at the review prompt.
fn answered_then(after: &str) -> String {
    format!("{PASTED}{SERVICES}{after}")
}

/// The three answers that finish a run on the pasted-database route: local
/// embedding, no reranking, and extraction skipped.
const FINISHES: &str = "1\n3\n1\n";

#[test]
fn a_finished_run_answers_with_a_preview_and_no_signal() {
    let run = Harness::fresh()
        .with_pasted_secrets(&["pw"])
        .typing(PASTED)
        .typing(FINISHES)
        .drive();

    assert!(run.signal().is_none(), "{:?}\n{}", run.signal(), run.err);
    // `answers` panics when the run did not finish, so reaching it is the check.
    let _ = run.answers();
    assert!(!run.view().blocks.is_empty(), "a preview was built");
    assert!(
        run.preview_text().contains("Clio setup"),
        "{}",
        run.preview_text()
    );
}

#[test]
fn a_refused_run_reports_its_signal_and_panics_on_the_answers() {
    // The two accessors have different contracts on purpose: `signal` is the way
    // to ask about a run that did not finish, and `answers` says so loudly rather
    // than handing back a half-built value.
    let run = Harness::fresh()
        .with_effective(serde_json::json!({ "sync": { "omit": false } }))
        .flag("sync-store", "smartcard")
        .typing(FINISHES)
        .drive();

    assert!(run.signal().is_some(), "a refusal carries a signal");
    assert!(run.view.is_none(), "a refused run builds no preview");
    // The panic message is captured rather than printed, so a caught refusal does
    // not put a backtrace on a green run's output.
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = run.answers();
    }));
    std::panic::set_hook(previous);
    assert!(
        panicked.is_err(),
        "answers() must not hand back a value for a run that stopped"
    );
}

#[test]
fn the_review_mode_hands_back_a_verdict_and_a_preview() {
    // The review loop is the other drive, and it is the one that records what the
    // operator chose at the prompt. Quitting is the default at that prompt, so no
    // answer is typed for it.
    let run = Harness::fresh()
        .with_pasted_secrets(&["pw"])
        .typing(answered_then("y\n"))
        .review();

    assert!(run.signal().is_none(), "{:?}\n{}", run.signal(), run.err);
    assert_eq!(run.verdict, Some(Verdict::Apply), "{}", run.out);
    let view = run.view();
    assert!(!view.blocks.is_empty(), "{:?}", view.blocks);
}

#[test]
fn a_refused_review_reports_the_signal_it_stopped_on() {
    let run = Harness::fresh()
        .with_effective(serde_json::json!({ "sync": { "omit": false } }))
        .flag("sync-store", "smartcard")
        .typing(FINISHES)
        .review();

    let signal = run.signal().expect("the refusal stopped the run");
    assert!(
        matches!(signal, Signal::Usage(reason) if reason.contains("--sync-store must be file|keystore")),
        "{signal:?}"
    );
    assert!(
        run.verdict.is_none(),
        "no verdict before a preview is agreed"
    );
}
