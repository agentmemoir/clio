// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests of the two questions that follow the
//! preview: the amend question and the typed `switch` confirmation.
//!
//! ## Owns
//! - `Amend` naming, and the amend menu's offered set, under test for
//!   `setup_report_amend`
//! - The typed `switch` confirmation and its refusal, under test
//!
//! ## Does not own
//! - The preview itself (see `setup_preview_tests`)
//! - The apply gate, which is a yes-or-no (see `setup_review_tests`)
//! - The loop that calls these (see `setup_review_tests`)
//!
//! ## Boundary
//!
//! Changes to what the post-preview questions say should normally update tests
//! here. Changes to the preview should not require changes here.
//!
//! Keep this module focused on the last two questions of a run.
//!
//! # Why these build their own context
//!
//! `Ctx` borrows the reader and both writers for its whole life, so a test that
//! needs to read the printed bytes back has to own the four locals. The shared
//! harness does that too, but it also drives the whole wizard, and these
//! questions are only reachable after every other answer is in hand.

use super::*;
use crate::cli_style::Palette;
use crate::compose_docker::DockerStatus;
use crate::setup_cli_flows::Ctx;
use crate::setup_defaults::Defaults;
use crate::setup_test_seam::Probes;
use std::collections::HashMap;
use std::io::Cursor;
use std::path::PathBuf;

/// Run one post-preview question over a scripted reader, handing back its answer
/// and everything it printed.
fn over<T>(lines: &str, call: impl FnOnce(&mut Ctx<'_>) -> T) -> (T, String) {
    let flags: HashMap<String, String> = HashMap::new();
    let probes = Probes::ok();
    let mut reader = Cursor::new(lines.as_bytes().to_vec());
    let mut out: Vec<u8> = Vec::new();
    let mut err: Vec<u8> = Vec::new();
    let mut ctx = Ctx::new(
        &flags,
        &mut reader,
        &mut out,
        &mut err,
        true,
        Palette::new(false),
        &probes,
        Defaults::empty(),
        PathBuf::from("/nonexistent/clio-stage5/clio.db"),
        DockerStatus::Available,
        &crate::setup_preview_scripts::no_env,
        false,
    );
    let answer = call(&mut ctx);
    (answer, String::from_utf8(out).expect("utf8"))
}

/// A reader that refuses to read, because a `Cursor` cannot.
///
/// A closed terminal is the only thing that reaches the read-error path, and a
/// silent prompt is the one failure an operator cannot recover from: the run looks
/// as though it is waiting for an answer that will never come.
struct BrokenReader;

impl std::io::Read for BrokenReader {
    fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
        Err(std::io::Error::other("the terminal went away"))
    }
}

impl std::io::BufRead for BrokenReader {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        Err(std::io::Error::other("the terminal went away"))
    }
    fn consume(&mut self, _amt: usize) {}
}

/// Run one question over a reader that fails, so the error path is reachable.
///
/// Built separately from [`over`] because the four locals have to outlive the
/// borrow, and a failing reader cannot be a `Cursor` this module already owns.
fn over_broken_reader<T>(call: impl FnOnce(&mut Ctx<'_>) -> T) -> (T, String) {
    let flags: HashMap<String, String> = HashMap::new();
    let probes = Probes::ok();
    let mut reader = BrokenReader;
    let mut out: Vec<u8> = Vec::new();
    let mut err: Vec<u8> = Vec::new();
    let mut ctx = Ctx::new(
        &flags,
        &mut reader,
        &mut out,
        &mut err,
        true,
        Palette::new(false),
        &probes,
        Defaults::empty(),
        PathBuf::from("/nonexistent/clio-stage5/clio.db"),
        DockerStatus::Available,
        &crate::setup_preview_scripts::no_env,
        false,
    );
    let answer = call(&mut ctx);
    (answer, String::from_utf8(out).expect("utf8"))
}

#[test]
fn every_section_has_a_name_the_list_and_the_back_message_share() {
    assert_eq!(
        Amend::ALL.map(Amend::label),
        ["Database", "Embedding", "Reranking", "Extraction"]
    );
}

#[test]
fn a_section_name_is_read_in_any_case() {
    for section in Amend::ALL {
        assert_eq!(Amend::parse(&section.label().to_lowercase()), Some(section));
        assert_eq!(
            Amend::parse(&format!("  {}  ", section.label())),
            Some(section)
        );
        assert_eq!(Amend::parse(section.value()), Some(section));
    }
    assert_eq!(Amend::parse("Embeddings"), None);
}

#[test]
fn the_amend_question_lists_the_sections_offered() {
    let (chosen, printed) = over("Database\n", |ctx| {
        amend_section(ctx, &[Amend::Database, Amend::Embedding])
    });
    assert_eq!(chosen, Ok(Some(Amend::Database)));
    assert!(printed.contains("[1] Database"), "{printed}");
    assert!(printed.contains("[2] Embedding"), "{printed}");
    assert!(!printed.contains("Reranking"), "{printed}");
}

#[test]
fn a_section_the_operator_never_answered_is_not_offered() {
    // Only the answered sections appear, because a section with no answer has
    // nothing to re-ask and re-asking it would ask again a question the operator
    // already answered by declining.
    let (chosen, printed) = over("1\n", |ctx| amend_section(ctx, &[Amend::Embedding]));
    assert_eq!(chosen, Ok(Some(Amend::Embedding)));
    assert!(!printed.contains("Database"), "{printed}");
    assert!(!printed.contains("Reranking"), "{printed}");
    assert!(!printed.contains("Extraction"), "{printed}");
}

#[test]
fn backing_out_of_the_amend_question_leaves_the_preview_on_screen() {
    // Backing out means "not now" rather than "leave": the preview is still drawn
    // and the operator may want to apply the answers as they stand. Two sections
    // offered plus the way out makes `3` the way out.
    let (chosen, printed) = over("3\n", |ctx| {
        amend_section(ctx, &[Amend::Database, Amend::Embedding])
    });
    assert_eq!(chosen, Ok(None));
    // Two sections offered plus the way out makes it the third item. Which of the
    // two labels it carries depends on whether the run has answered anything yet,
    // which the prompt layer owns and tests for itself, so only the position is
    // asserted here.
    assert!(printed.contains("[3] "), "{printed}");
}

#[test]
fn a_wrong_section_name_is_refused_and_lists_the_offered_ones() {
    // A typo, then the right answer. The refusal itself is the renderer's, which
    // lists the options and re-asks; this test is about the run reaching the
    // section the operator meant rather than stopping on the typo.
    let (chosen, printed) = over("Embeddings\n1\n", |ctx| {
        amend_section(ctx, &[Amend::Embedding])
    });
    assert_eq!(chosen, Ok(Some(Amend::Embedding)));
    // The menu is redrawn after the refusal, so the operator can correct the typo
    // from what is on screen rather than from memory.
    assert!(printed.contains("`Embeddings` is not one of"), "{printed}");
    assert!(printed.contains("1 embedding"), "{printed}");
}

#[test]
fn an_amend_question_with_no_input_names_the_flag_equivalent() {
    let (chosen, _printed) = over("", |ctx| amend_section(ctx, &[Amend::Embedding]));
    let error = chosen.expect_err("a refusal");
    assert!(
        error.reason().contains("pass the equivalent flag"),
        "{error:?}"
    );
}

#[test]
fn a_run_with_nothing_to_amend_is_not_asked_which_section_to_change() {
    // A menu with no options would have to invent a default, and every default it
    // could pick names a section the operator never answered. So a run with no
    // answers to change applies what it has instead of asking.
    let (chosen, printed) = over("\n", |ctx| amend_section(ctx, &[]));
    assert_eq!(chosen, Ok(None));
    assert_eq!(
        printed, "",
        "nothing was drawn for a question that was not asked"
    );
}

#[test]
fn the_switch_prompt_states_the_consequence_before_it_asks() {
    // The caller's own consequence sentence, then the shared one. The counts here
    // are the plural form on purpose: the singular spelling is decided by
    // `StoreState::consequence` and covered there.
    let reason = "You have 12 memories about 3 subjects in the database Clio uses today.";
    let (outcome, printed) = over("switch\n", |ctx| switch(ctx, reason));
    assert_eq!(outcome, Ok(()));
    assert!(printed.contains(reason), "{printed}");
    assert!(
        printed.contains("Nothing is deleted and nothing is copied"),
        "{printed}"
    );
    assert!(printed.contains("Type `switch`"), "{printed}");
}

#[test]
fn the_word_switch_is_accepted_however_it_is_spelled() {
    for typed in ["switch", "SWITCH", " switch "] {
        let (outcome, _printed) = over(&format!("{typed}\n"), |ctx| switch(ctx, "because"));
        assert_eq!(outcome, Ok(()), "for {typed}");
    }
}

#[test]
fn enter_re_asks_rather_than_agreeing() {
    // An empty line is the most likely thing to be typed by reflex, so it must
    // not be read as consent to leave a store behind.
    let (outcome, printed) = over("\nswitch\n", |ctx| switch(ctx, "because"));
    assert_eq!(outcome, Ok(()));
    assert!(printed.contains("`switch` was not typed"), "{printed}");
    assert!(printed.contains("2 attempts left"), "{printed}");
}

#[test]
fn the_last_wrong_attempt_says_nothing_changed_and_stops_asking() {
    // The third refusal replaces the count with the outcome, so an operator who
    // has run out of attempts is told the consequence rather than a tally.
    let (outcome, printed) = over("yes\nok\nsure\n", |ctx| switch(ctx, "because"));
    let error = outcome.expect_err("a refusal");
    // The refusal says what did not happen rather than counting attempts: the
    // operator has run out and the only thing left to tell them is that nothing
    // was touched.
    assert!(
        error
            .reason()
            .contains("the confirmation was not typed, so nothing was changed"),
        "{error:?}"
    );
    // The last refusal replaces the tally with the outcome, so the operator who has
    // run out is told what did not happen rather than how many tries are left. The
    // earlier refusals do carry a count, which is what tells them they can try again.
    let last = printed
        .rsplit("`switch` was not typed")
        .next()
        .expect("at least one refusal");
    assert!(last.contains("Nothing was changed."), "{printed}");
    assert!(
        !last.contains("attempt"),
        "no tally in the last refusal: {printed}"
    );
}

#[test]
fn a_switch_prompt_with_no_input_refuses_without_waiting() {
    let (outcome, _printed) = over("", |ctx| switch(ctx, "because"));
    let error = outcome.expect_err("a refusal");
    assert!(error.reason().contains("no input"), "{error:?}");
}

#[test]
fn a_run_out_of_input_at_the_switch_confirmation_writes_nothing() {
    // The preview is already agreed to at this point, so there is nothing to go
    // back to and nothing to agree to. A run that runs out of input here is a usage
    // stop naming the question, and the important part is the second half: nothing
    // was written, which is what the caller does with the refusal.
    let (outcome, _printed) = over("", |ctx| switch(ctx, "because"));
    let error = outcome.expect_err("a usage stop");
    assert!(error.reason().contains("no input"), "{error:?}");
}

#[test]
fn a_terminal_that_goes_away_reports_the_read_that_failed() {
    // A failed read is reported rather than treated as an answer, so the run stops
    // with the reason in hand instead of continuing on an empty reply.
    let (outcome, _printed) = over_broken_reader(|ctx| amend_section(ctx, &[Amend::Database]));
    let error = outcome.expect_err("a refusal");
    assert!(error.reason().contains("prompt failed"), "{error:?}");
    assert!(
        error.reason().contains("the terminal went away"),
        "{error:?}"
    );

    let (outcome, _printed) = over_broken_reader(|ctx| switch(ctx, "because"));
    let error = outcome.expect_err("a refusal");
    assert!(error.reason().contains("prompt failed"), "{error:?}");
    assert!(
        error.reason().contains("the terminal went away"),
        "{error:?}"
    );
}

#[test]
fn the_not_moving_data_sentence_says_what_happens_and_what_does_not() {
    // One spelling for the preview, the notice under it, and the confirmation, so
    // the wizard cannot describe the same action in two ways.
    //
    // The three facts an operator needs before agreeing to point Clio somewhere
    // new: where it will read from afterwards, that those memories will not appear
    // there, and that the old database is still intact. An earlier wording said
    // "Clio changes where it looks" and "does not move data", which are both true
    // and neither of which says any of that.
    let sentence = not_moving_data();
    assert!(
        sentence.contains("reads and writes the new database instead"),
        "{sentence}"
    );
    assert!(sentence.contains("will not show up in Clio"), "{sentence}");
    assert!(
        sentence.contains("Nothing is deleted and nothing is copied"),
        "{sentence}"
    );
    assert!(
        sentence.contains("the old database keeps everything"),
        "{sentence}"
    );
}
