// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests of the one-question layer: which
//! reply or signal each kind of question hands back, on a terminal and with a
//! reader, and what a prompt failure becomes.
//!
//! ## Owns
//! - Question behavior under test for `setup_ask`
//!
//! ## Does not own
//! - The plain renderer's own bytes and layout (see `setup_render_tests`)
//! - The terminal prompts themselves (see `setup_prompt`)
//! - Which questions exist and in what order (see `setup_steps_tests`)
//!
//! ## Boundary
//!
//! Changes to how a question's answer becomes a `Reply` or a `Signal` should
//! normally update tests here. Changes to what a question says, or to how it
//! looks, should not require changes here.
//!
//! Keep this module focused on one question and one answer.
//!
//! # Why the terminal tests pass a guard
//!
//! The prompt library draws to the process's error stream and refuses to read a
//! key when that stream is not a terminal, so a test runner's pipe makes every
//! prompt return at once. A developer running `cargo test` from a real terminal
//! would instead get a prompt that waits for a keypress, so each terminal test
//! checks [`no_terminal`] first and does nothing when there is one.
//!
//! # Why one arm of the plain yes-or-no question has no test
//!
//! The plain renderer's free-text question reads a line and hands the value
//! back; its only other outcome is a failure. So the arm that would turn a back
//! into "nothing" cannot be reached from a reader, and reaching it would mean
//! adding a hook no run uses. It stays uncovered on purpose.

use super::*;
use crate::setup_defaults::Defaults;
use crate::setup_prompt::{FIELD_KEYS, QUIT_LABEL};
use std::io::{Cursor, IsTerminal};
use std::path::PathBuf;

/// The one secret the masked reader hands out.
const SECRET: &str = "correct-horse";

/// Two menu options, in the wizard's own wording.
///
/// A function rather than a constant because `Choice::new` is not `const`, and a
/// test that rebuilt the list inline would let the wording drift between tests.
fn choices() -> [Choice; 2] {
    [
        Choice::new(
            "managed",
            "Clio Manages Postgres in Docker",
            "Clio starts the database for you",
        ),
        Choice::new("external", "An existing Postgres", "you already run one"),
    ]
}

/// A context over a scripted reader and two byte buffers, with the terminal flag
/// the test names.
///
/// The four locals are the ones `Ctx` borrows for its whole life, so a test that
/// reads through the context has to declare them the same way the harness does
/// rather than reaching into a struct.
///
/// Docker is available and there is no terminal, which is the pair a scripted
/// test wants: the managed and external choices have to be on offer for a test to
/// get past the first question, and the plain renderer is the one an in-memory
/// reader can be driven through at all.
macro_rules! with_ctx {
    ($input:expr, $terminal:expr, |$ctx:ident| $body:expr) => {{
        let mut input = Cursor::new($input.as_bytes().to_vec());
        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let flags = std::collections::HashMap::new();
        let probes = crate::setup_test_seam::Probes::ok();
        let mut $ctx = Ctx::new(
            &flags,
            &mut input,
            &mut out,
            &mut err,
            true,
            crate::cli_style::Palette::new(false),
            &probes,
            Defaults::empty(),
            PathBuf::from("/nonexistent/clio-setup-ask/clio.db"),
            crate::compose_docker::DockerStatus::Available,
            &crate::setup_preview_scripts::no_env,
            $terminal,
        );
        $ctx = $ctx.with_secrets(crate::setup_test_seam::scripted_secrets(vec![
            SECRET.to_owned(),
        ]));
        let answer = $body;
        (answer, String::from_utf8(out).expect("utf8"))
    }};
}

/// True when the error stream is a pipe, so a prompt returns at once instead of
/// waiting for a keypress. A test runner and CI both give the test binary a pipe,
/// so the terminal paths really do run there; a developer running `cargo test`
/// from a terminal gets the early return instead of a hang.
fn no_terminal() -> bool {
    !std::io::stderr().is_terminal()
}

/// The plain renderer's reading of one typed line on the yes-or-no question.
fn yes_no(answer: &str) -> Result<Option<bool>, Signal> {
    with_ctx!(&format!("{answer}\n"), false, |ctx| ctx
        .ask_yes_no("Write these settings and start the containers?"))
    .0
}

#[test]
fn a_text_question_on_a_terminal_with_nothing_behind_it_goes_back() {
    if !no_terminal() {
        return;
    }
    let (answer, printed) = with_ctx!("", true, |ctx| ctx.ask("Project name", Some("agentmemoir")));
    assert_eq!(answer, Ok(Reply::Back), "{printed}");
    // The question was drawn on the wizard's own writer, which is why the hint is
    // in the captured bytes rather than on the terminal.
    assert!(printed.contains(FIELD_KEYS), "{printed}");
}

#[test]
fn a_yes_no_question_on_a_terminal_with_nothing_behind_it_agrees_to_nothing() {
    if !no_terminal() {
        return;
    }
    let (answer, printed) = with_ctx!("", true, |ctx| {
        ctx.ask_yes_no("Write these settings and start the containers?")
    });
    assert_eq!(answer, Ok(None), "{printed}");
}

#[test]
fn a_menu_on_a_terminal_with_nothing_behind_it_goes_back() {
    if !no_terminal() {
        return;
    }
    let (answer, printed) = with_ctx!("", true, |ctx| ctx.ask_choice(
        "Database",
        &choices(),
        "managed"
    ));
    assert_eq!(answer, Ok(Reply::Back), "{printed}");
}

#[test]
fn a_re_asking_question_on_a_terminal_with_nothing_behind_it_is_a_back() {
    if !no_terminal() {
        return;
    }
    let (answer, printed) = with_ctx!("", true, |ctx| {
        ctx.ask_checked("Project name", Some("agentmemoir"), |value: &str| {
            if value.is_empty() {
                Err("a name is required".to_owned())
            } else {
                Ok(())
            }
        })
    });
    assert_eq!(answer, Err(Signal::Back), "{printed}");
}

#[test]
fn a_secret_on_a_terminal_with_nothing_behind_it_is_a_back() {
    if !no_terminal() {
        return;
    }
    let (answer, printed) = with_ctx!("", true, |ctx| ctx.ask_password("Database password"));
    assert_eq!(answer, Err(Signal::Back), "{printed}");
}

#[test]
fn a_terminal_prompt_that_answered_hands_the_value_back_on_the_wizards_writer() {
    let (answer, printed) = with_ctx!("", true, |ctx| {
        let outcome: Result<Option<String>, Signal> = ctx.ask_on_terminal(|out: &mut dyn Write| {
            let _ = write!(out, "asked on the wizard's writer");
            Ok(Some("typed".to_owned()))
        });
        outcome
    });
    assert_eq!(answer, Ok(Some("typed".to_owned())), "{printed}");
    assert_eq!(printed, "asked on the wizard's writer", "{printed}");
}

#[test]
fn a_clean_failure_from_a_terminal_prompt_is_a_cancel_and_not_a_refusal() {
    let (answer, printed) = with_ctx!("", true, |ctx| {
        let outcome: Result<Option<String>, Signal> =
            ctx.ask_on_terminal(|_out: &mut dyn Write| Err(CliError::clean()));
        outcome
    });
    assert_eq!(answer, Ok(None), "{printed}");
}

#[test]
fn a_failed_terminal_prompt_is_a_refusal_and_not_a_cancel() {
    let (answer, printed) = with_ctx!("", true, |ctx| {
        let outcome: Result<Option<String>, Signal> =
            ctx.ask_on_terminal(|_out: &mut dyn Write| {
                Err(CliError::failed("the question could not be asked"))
            });
        outcome
    });
    // A refusal is what leaves the process with exit code 1; a cancel would be a
    // clean exit with no message at all.
    assert_eq!(
        answer,
        Err(Signal::Refused(
            "the question could not be asked".to_owned()
        )),
        "{printed}"
    );
}

#[test]
fn a_typed_line_becomes_the_value_and_clears_the_flag_that_says_a_back_leaves() {
    let (answer, printed) = with_ctx!("clio\n", false, |ctx| {
        assert!(ctx.back_leaves_run(), "the first question of a run");
        ctx.ask("Project name", Some("agentmemoir"))
    });
    assert_eq!(answer, Ok(Reply::Value("clio".to_owned())), "{printed}");
    assert!(printed.contains("Project name"), "{printed}");
}

#[test]
fn a_reader_with_nothing_left_in_it_is_a_usage_stop_and_not_a_back() {
    let (answer, printed) = with_ctx!("", false, |ctx| ctx.ask("Project name", None));
    // A usage stop is what leaves the process with exit code 2, so the message
    // names the flag that answers the question without a prompt.
    assert_eq!(
        answer,
        Err(Signal::Usage(
            "no input for `Project name`; pass the equivalent flag".to_owned()
        )),
        "{printed}"
    );
}

#[test]
fn a_menu_number_becomes_the_value_and_clears_the_flag_that_says_a_back_leaves() {
    let (answer, printed) = with_ctx!("2\n", false, |ctx| {
        assert!(ctx.back_leaves_run(), "the first question of a run");
        ctx.ask_choice("Database", &choices(), "managed")
    });
    assert_eq!(answer, Ok(Reply::Value("external".to_owned())), "{printed}");
}

#[test]
fn the_first_questions_way_out_of_a_menu_leaves_the_run_and_keeps_the_flag() {
    let (answer, printed) = with_ctx!("3\n", false, |ctx| {
        let outcome = ctx.ask_choice("Database", &choices(), "managed");
        (outcome, ctx.back_leaves_run())
    });
    assert_eq!(answer.0, Ok(Reply::Back), "{printed}");
    assert!(answer.1, "a back must leave the flag alone");
    assert!(printed.contains(QUIT_LABEL), "{printed}");
}

#[test]
fn a_re_asking_question_returns_the_first_line_its_rule_accepts() {
    let (answer, printed) = with_ctx!("\nno\nAgent Memoir\n", false, |ctx| {
        assert!(ctx.back_leaves_run(), "the first question of a run");
        let outcome = ctx.ask_checked("Project name", Some("agentmemoir"), |value: &str| {
            if value.eq_ignore_ascii_case("agent memoir") {
                Ok(())
            } else {
                Err("type Agent Memoir".to_owned())
            }
        });
        (outcome, ctx.back_leaves_run())
    });
    assert_eq!(answer.0, Ok("Agent Memoir".to_owned()), "{printed}");
    assert!(!answer.1, "an answered field clears the flag");
    // Two lines were refused, so the question was drawn three times.
    assert_eq!(printed.matches("type Agent Memoir").count(), 2, "{printed}");
}

#[test]
fn a_secret_comes_from_the_masked_reader_and_never_lands_in_the_output() {
    let (answer, printed) = with_ctx!("", false, |ctx| ctx.ask_password("Database password"));
    assert_eq!(answer, Ok(SECRET.to_owned()), "{printed}");
    assert!(printed.contains("Database password"), "{printed}");
    assert!(!printed.contains(SECRET), "{printed}");
}

#[test]
fn only_the_words_that_mean_yes_read_as_yes() {
    for answer in ["y", "yes", "YES", " Y "] {
        assert_eq!(yes_no(answer), Ok(Some(true)), "{answer:?}");
    }
}

#[test]
fn anything_else_reads_as_no() {
    for answer in ["", "n", "yep", "no thanks"] {
        assert_eq!(yes_no(answer), Ok(Some(false)), "{answer:?}");
    }
}
