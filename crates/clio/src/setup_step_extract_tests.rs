// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the extraction question: its flag, its
//! not-available notice for hosted extraction, and the skip a non-interactive run
//! records without the flag.
//!
//! ## Owns
//! - Every test of `setup_step_extract`, by flag, non-interactively, and through
//!   the interactive menu
//!
//! ## Does not own
//! - The other step questions (see `setup_step_ask_tests`)
//! - Navigation between steps (see `setup_steps_tests`)
//!
//! ## Boundary
//!
//! Changes to what the extraction question asks should normally update tests
//! here. Changes to the rest of the wizard should not.
//!
//! Keep this module focused on the extraction question.

use crate::setup_preview_fixtures::{Harness, PASTED, SERVICES, SQLITE};
use crate::setup_services::ServiceRole;

#[test]
fn the_extract_flag_records_a_local_or_skipped_role() {
    let local = Harness::fresh()
        .flag("extract", "local")
        .typing(SQLITE)
        .typing(SERVICES)
        .run();
    assert_eq!(local.answers().extract, Some(ServiceRole::Local));

    let skipped = Harness::fresh()
        .flag("extract", "skip")
        .typing(SQLITE)
        .typing(SERVICES)
        .run();
    assert_eq!(skipped.answers().extract, Some(ServiceRole::Skip));
}

#[test]
fn a_non_interactive_run_without_the_flag_skips_extraction() {
    let run = Harness::fresh()
        .non_interactive()
        .flag("database", "sqlite")
        .flag("embed", "local")
        .flag("rerank", "skip")
        .run();

    assert_eq!(
        run.answers().extract,
        Some(ServiceRole::Skip),
        "{}",
        run.out
    );
}

// ---------------------------------------------------- the interactive menu

/// The three answers that reach the extraction menu: local embeddings, no
/// reranking, then the extraction question itself. The database is answered by
/// the pasted route.
const TO_THE_MENU: &str = "1\n3\n";

#[test]
fn the_extraction_menu_offers_the_three_options_and_defaults_to_skip() {
    // Extraction's menu is local, hosted, skip, and the highlight starts on skip:
    // a hosted extractor is not built yet, so the option that works everywhere is
    // the one an untouched question should offer. This is the interactive half of
    // the question, so it is driven here rather than through the flag.
    let run = Harness::fresh()
        .with_pasted_secrets(&["pw"])
        .typing(PASTED)
        .typing(TO_THE_MENU)
        // The default is what this test is about, so it is taken rather than
        // typed: an untouched run must land on skip without an answer.
        .typing("\n")
        .run();

    assert!(run.out.contains("[1] Local Docker"), "{}", run.out);
    assert!(
        run.out.contains("[2] Hosted (not available yet)"),
        "{}",
        run.out
    );
    assert!(run.out.contains("[3] Skip"), "{}", run.out);
    assert!(
        run.out.contains("press Enter for [skip]"),
        "the default is skip: {}",
        run.out
    );
    assert_eq!(
        run.answers().extract,
        Some(ServiceRole::Skip),
        "pressing Enter takes the default: {}",
        run.out
    );
}

#[test]
fn choosing_local_at_the_menu_records_a_local_role() {
    let run = Harness::fresh()
        .with_pasted_secrets(&["pw"])
        .typing(PASTED)
        .typing(TO_THE_MENU)
        .typing("1\n")
        .run();

    assert_eq!(run.answers().extract, Some(ServiceRole::Local));
}

#[test]
fn hosted_at_the_menu_prints_the_notice_and_writes_no_config_entry() {
    // A preview promising a hosted extractor would be a lie, so choosing it
    // prints the notice and records the skip. The flag way round is the test
    // above; this one drives the menu itself.
    let run = Harness::fresh()
        .with_pasted_secrets(&["pw"])
        .typing(PASTED)
        .typing(TO_THE_MENU)
        .typing("2\n")
        .run();

    assert!(
        run.out.contains("hosted extraction is not available yet"),
        "{}",
        run.out
    );
    assert_eq!(run.answers().extract, Some(ServiceRole::Skip));
    let entries = crate::setup_preview::planned_entries(run.answers());
    assert!(
        !entries.iter().any(|(path, _)| path.starts_with("extract")),
        "{entries:?}"
    );
}

#[test]
fn going_back_at_the_extraction_question_re_enters_it() {
    // `b` at the extraction menu goes back one step, which is the reranking
    // question, and answering it forward re-enters the menu. No untouched run
    // takes this branch, so without it the walk-back arm is unmeasured.
    let run = Harness::fresh()
        .with_pasted_secrets(&["pw"])
        .typing(PASTED)
        .typing(TO_THE_MENU)
        .typing("b\n")
        .typing("3\n")
        .typing("2\n")
        .run();

    assert!(
        run.out.contains("Where should extraction come from?"),
        "the walk back re-enters the question: {}",
        run.out
    );
    assert_eq!(run.answers().extract, Some(ServiceRole::Skip));
}
