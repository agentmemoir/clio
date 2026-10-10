// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests of the terminal prompts: the key hint
//! above a question, the menu, the field, the checked field, the secret, the
//! yes-or-no, and the way a prompt that cannot be asked becomes the wizard's own
//! error.
//!
//! ## Owns
//! - Render behavior and return values under test for `setup_prompt`
//! - The mapping from a chosen menu value to the wizard's own answer
//!
//! ## Does not own
//! - The plain-text renderer the colour-off runs use (see `setup_render_tests`)
//! - Which questions exist and in what order (see `setup_steps_tests`)
//! - What a valid answer is (see `setup_validate_tests`)
//!
//! ## Boundary
//!
//! Changes to how a question is drawn on a terminal, or to the back rule, should
//! normally update tests here. Changes to what is asked should not require changes
//! here.
//!
//! Keep this module focused on drawing one question and reading one answer.
//!
//! # Why every prompt test here stops without a terminal
//!
//! The prompt library draws to stderr and reads keys from the terminal. With no
//! terminal at that end it returns at once, which is the whole reason the error
//! path can be tested without a keypress. With a real terminal it waits for a
//! key instead, so a test that called a prompt unguarded would hang a developer
//! running `cargo test` from a terminal. Every prompt test therefore returns early
//! when stderr is a terminal, and asserts in full when stderr is a pipe, which is
//! what the test runner and CI hand the test binary.
//!
//! # Why the hint is compared byte for byte
//!
//! The hint is drawn dimmed and on its own line, and both are the contract: a hint
//! written into the prompt line would be frozen into the transcript of every
//! answer. Comparing the bytes, dim codes and trailing newline included, pins the
//! layout and not only the words.

use super::*;
use std::fmt::Debug;

/// The exact menu hint line: two spaces, the keys dimmed, then a newline.
const MENU_HINT_LINE: &str = "  \x1b[2m↑ ↓ move · Enter take · Esc go back\x1b[0m\n";

/// The exact field hint line, in the same shape as the menu's.
const FIELD_HINT_LINE: &str = "  \x1b[2mEnter take · Esc go back\x1b[0m\n";

/// True when stderr is a pipe, so a prompt returns at once instead of waiting for
/// a keypress.
///
/// The test runner and CI give the test binary a pipe, so the prompt paths below
/// really do run there; a developer running `cargo test` from a terminal gets this
/// early return instead of a hang.
fn no_terminal() -> bool {
    !io::stderr().is_terminal()
}

/// Two real answers, so a menu test has options to draw.
fn options() -> [Choice; 2] {
    [
        Choice::new(
            "managed",
            "Clio Manages Postgres in Docker",
            "no password to remember",
        ),
        Choice::new(
            "sqlite",
            "A SQLite file on this machine",
            "one file, no Docker",
        ),
    ]
}

/// The error a prompt returned instead of an answer.
fn closed_terminal<T: Debug>(answer: Result<T, PromptError>) -> PromptError {
    answer.expect_err("a prompt with no terminal has no answer to give")
}

/// The clean exit a prompt failure becomes: code 0 and nothing to say.
fn assert_clean(error: &PromptError) {
    assert_eq!(
        (error.code, error.message.as_str()),
        (0, ""),
        "a closed terminal is a clean exit, not a refusal"
    );
}

/// Everything a prompt wrote to the caller's writer, as text.
fn printed_as_text(out: Vec<u8>) -> String {
    String::from_utf8(out).expect("the hint is utf8")
}

#[test]
fn the_hint_is_two_spaces_a_dimmed_line_and_a_newline() {
    let mut out: Vec<u8> = Vec::new();
    hint(&mut out, MENU_KEYS);
    assert_eq!(printed_as_text(out), MENU_HINT_LINE);
}

#[test]
fn a_field_names_the_keys_that_work_in_a_field() {
    // The menu keys would tell the operator to move with arrows, which a field
    // cannot do, so the two lines are checked apart and not against each other.
    let mut out: Vec<u8> = Vec::new();
    hint(&mut out, FIELD_KEYS);
    assert_eq!(printed_as_text(out), FIELD_HINT_LINE);
}

#[test]
fn a_menu_draws_its_own_keys_before_the_question() {
    if !no_terminal() {
        return;
    }
    let mut out: Vec<u8> = Vec::new();
    let answer = menu(
        &mut out,
        "Where should your memories live?",
        &options(),
        "sqlite",
    );
    assert_eq!(printed_as_text(out), MENU_HINT_LINE);
    assert_clean(&closed_terminal(answer));
}

#[test]
fn a_field_with_a_default_draws_the_field_keys_before_the_question() {
    if !no_terminal() {
        return;
    }
    let mut out: Vec<u8> = Vec::new();
    let answer = text(&mut out, "Host", Some("127.0.0.1"));
    assert_eq!(printed_as_text(out), FIELD_HINT_LINE);
    assert_clean(&closed_terminal(answer));
}

#[test]
fn a_field_with_no_default_asks_the_same_question_the_same_way() {
    // The default is optional, so the field without one has to reach the terminal
    // the same way, or a required question would ask nothing at all.
    if !no_terminal() {
        return;
    }
    let mut out: Vec<u8> = Vec::new();
    let answer = text(&mut out, "Host", None);
    assert_eq!(printed_as_text(out), FIELD_HINT_LINE);
    assert_clean(&closed_terminal(answer));
}

#[test]
fn a_checked_field_with_a_default_reaches_the_terminal_with_its_rule() {
    // The rule is handed to the prompt rather than checked after it, so a field
    // with a default has to carry both or the check silently does not happen.
    if !no_terminal() {
        return;
    }
    let mut out: Vec<u8> = Vec::new();
    let answer = checked_text(&mut out, "Port", Some("5432"), |value| {
        if value.chars().all(|c: char| c.is_ascii_digit()) {
            Ok(())
        } else {
            Err(format!("`{value}` is not a port"))
        }
    });
    assert_eq!(printed_as_text(out), FIELD_HINT_LINE);
    assert_clean(&closed_terminal(answer));
}

#[test]
fn a_checked_field_with_no_default_reaches_the_terminal_with_its_rule() {
    if !no_terminal() {
        return;
    }
    let mut out: Vec<u8> = Vec::new();
    let answer = checked_text(&mut out, "Port", None, |_| Ok(()));
    assert_eq!(printed_as_text(out), FIELD_HINT_LINE);
    assert_clean(&closed_terminal(answer));
}

#[test]
fn a_secret_reaches_the_terminal_without_printing_anything_it_typed() {
    // Nothing is typed here, because nothing can be asked: the point is that the
    // question is put to the terminal at all, and that nothing about the secret
    // reaches the caller's writer, which is where an unhidden prompt would show it.
    if !no_terminal() {
        return;
    }
    let mut out: Vec<u8> = Vec::new();
    let answer = secret(&mut out, "Database password");
    assert_eq!(printed_as_text(out), FIELD_HINT_LINE);
    assert_clean(&closed_terminal(answer));
}

#[test]
fn a_yes_or_no_reaches_the_terminal_and_returns_a_clean_exit() {
    if !no_terminal() {
        return;
    }
    let mut out: Vec<u8> = Vec::new();
    let answer = confirm(&mut out, "Let Clio write this config?");
    assert_eq!(printed_as_text(out), FIELD_HINT_LINE);
    assert_clean(&closed_terminal(answer));
}

#[test]
fn a_cancelled_question_is_a_clean_exit_with_nothing_to_say() {
    let cancelled = terminal_error(io::ErrorKind::Interrupted.into());
    assert_clean(&cancelled);
}

#[test]
fn a_dead_terminal_is_a_clean_exit_with_nothing_to_say() {
    // The library cannot tell a cancel from a closed terminal by shape alone, so
    // both land on the clean exit rather than on a refusal nobody can act on.
    let dead = terminal_error(io::ErrorKind::NotConnected.into());
    assert_clean(&dead);
}

#[test]
fn any_other_failure_names_what_could_not_be_asked() {
    let broken = terminal_error(io::Error::new(
        io::ErrorKind::PermissionDenied,
        "fd 2 is gone",
    ));
    assert_eq!(broken.code, 1, "a real failure is not a clean exit");
    assert_eq!(
        broken.message,
        "the question could not be asked: fd 2 is gone"
    );
}

#[test]
fn the_way_back_becomes_no_answer() {
    assert_eq!(answer_value(BACK_VALUE.to_owned()), None);
}

#[test]
fn a_real_value_stays_the_answer() {
    assert_eq!(answer_value("sqlite".to_owned()), Some("sqlite".to_owned()));
}

#[test]
fn only_the_whole_way_out_value_is_the_way_out() {
    // The comparison is exact on purpose: a question whose answer merely started
    // with the way out would lose a legitimate answer.
    let near = format!("{BACK_VALUE}-extra");
    assert_eq!(answer_value(near.clone()), Some(near));
}

#[test]
fn a_run_whose_stderr_is_a_pipe_is_not_asked_anything() {
    if !no_terminal() {
        return;
    }
    assert!(
        !terminal_available(),
        "a run that cannot be drawn on must not prompt"
    );
}

#[test]
fn the_way_out_is_appended_last_and_keeps_the_options_before_it() {
    let options = options();
    let all = with_back(&options, false);
    assert_eq!(all.len(), 3, "every option is kept: {}", all.len());
    assert_eq!(
        all[..2],
        options,
        "the question's own options keep their order"
    );
    let last = all.last().expect("the way out");
    assert_eq!(last.value, BACK_VALUE);
    assert_eq!(last.label, BACK_LABEL);
    assert_eq!(last.help, BACK_HINT);
}

#[test]
fn the_first_way_out_says_quit_rather_than_promising_a_return() {
    // There is nothing behind the first question, so promising a return would be a
    // lie. The value is the same either way, so a caller handles one reply.
    let all = with_back(&[], true);
    let last = all.last().expect("the way out");
    assert_eq!(last.value, BACK_VALUE);
    assert_eq!(last.label, QUIT_LABEL);
    assert_eq!(last.help, QUIT_HINT);
}

#[test]
fn an_option_keeps_its_three_parts() {
    let option = Choice::new("managed", "Clio Manages Postgres", "no password");
    assert_eq!(option.value, "managed");
    assert_eq!(option.label, "Clio Manages Postgres");
    assert_eq!(option.help, "no password");
}

#[test]
fn an_option_takes_anything_that_becomes_a_string() {
    let option = Choice::new(
        "sqlite",
        String::from("A SQLite file on this machine"),
        String::from("one file, no Docker"),
    );
    assert_eq!(option.label, "A SQLite file on this machine");
    assert_eq!(option.help, "one file, no Docker");
}

#[test]
fn a_note_is_bracketed_onto_the_label_and_touches_nothing_else() {
    let noted = Choice::new("managed", "Clio Manages Postgres", "no password")
        .with_note("Docker is running");
    assert_eq!(noted.label, "Clio Manages Postgres (Docker is running)");
    assert_eq!(noted.value, "managed");
    assert_eq!(noted.help, "no password");
}
