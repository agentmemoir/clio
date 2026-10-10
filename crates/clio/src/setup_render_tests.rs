// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests of the render helpers: the numbered
//! menu, the free-text field, the re-asking field, and the byte-for-byte output a
//! colour-off run produces.
//!
//! ## Owns
//! - Render behavior under test for `setup_render`
//!
//! ## Does not own
//! - The secret-reading seam (see `setup_mask_tests`)
//! - The terminal prompts (see `setup_prompt_tests`)
//! - Which questions exist (see `setup_steps_tests`)
//!
//! ## Boundary
//!
//! Changes to how a question looks or what it returns should normally update tests
//! here. Changes to a question should not require changes here.
//!
//! Keep this module focused on rendering and reading one answer.
//!
//! # Why every assertion here is colour-off
//!
//! `Palette::paint` returns its input untouched when colour is off, so a
//! colour-off run is plain text and a byte-exact assertion is possible. With
//! colour on the same assertion would be asserting an escape sequence, which
//! tests the palette rather than the question.

use super::*;
use crate::setup_prompt::{BACK_LABEL, BACK_VALUE, QUIT_LABEL};
use std::io::Cursor;

/// A reader holding exactly these lines.
fn input(lines: &str) -> Cursor<Vec<u8>> {
    Cursor::new(lines.as_bytes().to_vec())
}

/// Run one prompt and hand back both the answer and everything it printed.
fn run(
    lines: &str,
    ask: impl FnOnce(&mut Vec<u8>, &mut dyn BufRead) -> Result<Reply, CliError>,
) -> (Result<Reply, CliError>, String) {
    let mut out: Vec<u8> = Vec::new();
    let mut reader = input(lines);
    let answer = ask(&mut out, &mut reader);
    (answer, String::from_utf8(out).expect("utf8"))
}

/// Run one re-asking prompt, which answers with a value rather than a `Reply`.
fn run_checked(
    lines: &str,
    ask: impl FnOnce(&mut Vec<u8>, &mut dyn BufRead) -> Result<String, Signal>,
) -> (Result<String, Signal>, String) {
    let mut out: Vec<u8> = Vec::new();
    let mut reader = input(lines);
    let answer = ask(&mut out, &mut reader);
    (answer, String::from_utf8(out).expect("utf8"))
}

/// The three database options, in the wizard's own wording.
///
/// A function rather than a constant because `Choice::new` is not `const`, and a
/// test that rebuilt the list inline would let the wording drift between tests.
fn choices() -> [Choice; 3] {
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
        Choice::new(
            "existing",
            "A Postgres you already have",
            "an address and a password",
        ),
    ]
}

/// Ask the database menu, with the way out added as the renderer adds it.
fn database_menu(
    out: &mut Vec<u8>,
    reader: &mut dyn BufRead,
    default: &str,
    leaves: bool,
) -> Result<Reply, CliError> {
    ask_choice(
        out,
        reader,
        Palette::new(false),
        "Where should your memories live?",
        &choices(),
        default,
        leaves,
    )
}

#[test]
fn a_free_text_question_prints_its_label_and_its_default() {
    let (answer, printed) = run("db.example.com\n", |out, reader| {
        ask(out, reader, Palette::new(false), "Host", Some("127.0.0.1"))
    });
    assert_eq!(
        answer.expect("an answer"),
        Reply::Value("db.example.com".to_owned())
    );
    assert_eq!(printed, "Host ([127.0.0.1]): ");
}

#[test]
fn an_empty_line_takes_the_default() {
    let (answer, _printed) = run("\n", |out, reader| {
        ask(out, reader, Palette::new(false), "Port", Some("5432"))
    });
    assert_eq!(answer.expect("an answer"), Reply::Value("5432".to_owned()));
}

#[test]
fn a_question_with_no_default_refuses_an_empty_line() {
    let (answer, _printed) = run("\n", |out, reader| {
        ask(out, reader, Palette::new(false), "Host", None)
    });
    let error = answer.expect_err("a refusal");
    assert_eq!(error.code, 2, "an empty required field is a usage error");
    assert!(error.message.contains("Host"), "{}", error.message);
}

#[test]
fn a_free_text_field_takes_any_word_as_an_answer() {
    // The dialog keys are gone, and this is the consequence that matters: a field
    // that reserved `b` or `q` would have to refuse a legitimate one-character
    // answer. `b` is a real value for a boolean-shaped field and for an
    // application name, so it has to be read as one.
    for typed in ["b", "q"] {
        let (answer, _printed) = run(&format!("{typed}\n"), |out, reader| {
            ask(out, reader, Palette::new(false), "options", None)
        });
        assert_eq!(
            answer.expect("an answer"),
            Reply::Value(typed.to_owned()),
            "for {typed}"
        );
    }
}

#[test]
fn an_unanswered_question_names_the_equivalent_flag() {
    let (answer, _printed) = run("", |out, reader| {
        ask(out, reader, Palette::new(false), "Host", Some("h"))
    });
    let error = answer.expect_err("a refusal");
    assert_eq!(error.code, 2);
    assert!(
        error.message.contains("pass the equivalent flag"),
        "{}",
        error.message
    );
}

#[test]
fn a_numbered_question_prints_every_option_its_help_and_its_default() {
    // Byte-exact, because the layout is a contract: the section heading, one
    // indented line per option, the help under it, then the instruction naming
    // the default. A change here is a change to what the operator reads.
    let (answer, printed) = run("1\n", |out, reader| {
        database_menu(out, reader, "sqlite", true)
    });
    assert_eq!(
        answer.expect("an answer"),
        Reply::Value("managed".to_owned())
    );
    assert_eq!(
        printed,
        "\n  Where should your memories live?\n\
         \x20   [1] Clio Manages Postgres in Docker\n\
         \x20       no password to remember\n\
         \x20   [2] A SQLite file on this machine\n\
         \x20       one file, no Docker\n\
         \x20   [3] A Postgres you already have\n\
         \x20       an address and a password\n\
         \x20   [4] Quit\n\
         \x20       leave Clio without writing anything\n\
         \x20 Enter a number, or press Enter for [sqlite]:\n"
    );
}

#[test]
fn a_numbered_question_accepts_a_number() {
    for (typed, expected) in [("2", "sqlite"), ("3", "existing")] {
        let (answer, _printed) = run(&format!("{typed}\n"), |out, reader| {
            database_menu(out, reader, "managed", true)
        });
        assert_eq!(
            answer.expect("an answer"),
            Reply::Value(expected.to_owned()),
            "for {typed}"
        );
    }
}

#[test]
fn a_numbered_question_accepts_the_option_name_in_any_case() {
    let (answer, _printed) = run("ExIsTiNg\n", |out, reader| {
        database_menu(out, reader, "managed", true)
    });
    assert_eq!(
        answer.expect("an answer"),
        Reply::Value("existing".to_owned())
    );
}

#[test]
fn a_wrong_answer_re_asks_and_lists_the_options_again() {
    // The refusal prints the numbered list so a typo can be corrected from what is
    // on screen rather than from memory.
    let (answer, printed) = run("sideways\n3\n", |out, reader| {
        database_menu(out, reader, "managed", true)
    });
    assert_eq!(
        answer.expect("an answer"),
        Reply::Value("existing".to_owned())
    );
    assert!(printed.contains("`sideways` is not one of:"), "{printed}");
    assert!(printed.contains("1 managed"), "{printed}");
    assert!(printed.contains("2 sqlite"), "{printed}");
    assert!(printed.contains("3 existing"), "{printed}");
    // The way out is listed with the rest, so it is visible where the refusal is.
    assert!(printed.contains("4 --go-back"), "{printed}");
}

#[test]
fn the_way_out_is_the_last_option_and_says_go_back_once_answered() {
    // `leaves` is false once the run has an answer behind it, so the same option
    // promises a return rather than a departure.
    let (answer, printed) = run("4\n", |out, reader| {
        database_menu(out, reader, "managed", false)
    });
    assert_eq!(answer.expect("a reply"), Reply::Back);
    assert!(printed.contains(&format!("[4] {BACK_LABEL}")), "{printed}");
    assert!(!printed.contains(QUIT_LABEL), "{printed}");
}

#[test]
fn the_first_way_out_says_quit_rather_than_going_back() {
    // There is nothing behind the first question, so promising a return would be a
    // lie. The value is the same, so a caller handles one reply either way.
    let (answer, printed) = run("4\n", |out, reader| {
        database_menu(out, reader, "managed", true)
    });
    assert_eq!(answer.expect("a reply"), Reply::Back);
    assert!(printed.contains(&format!("[4] {QUIT_LABEL}")), "{printed}");
    assert!(!printed.contains(BACK_LABEL), "{printed}");
}

#[test]
fn the_way_out_carries_a_value_no_answer_can_collide_with() {
    // Every other answer is a bare word, so a value in double dashes cannot be one
    // a question offers. That is what lets one option serve as both the departure
    // and the return, and it is why typing it is the same as picking it.
    let (answer, _printed) = run(&format!("{BACK_VALUE}\n"), |out, reader| {
        database_menu(out, reader, "managed", true)
    });
    assert_eq!(answer.expect("a reply"), Reply::Back);
}

#[test]
fn a_checked_question_re_asks_until_the_value_is_accepted() {
    let (answer, printed) = run_checked("no\nnope\n5433\n", |out, reader| {
        ask_checked(out, reader, Palette::new(false), "Port", None, |value| {
            if value.chars().all(|c| c.is_ascii_digit()) {
                Ok(())
            } else {
                Err(format!("`{value}` is not a port"))
            }
        })
    });
    assert_eq!(answer.expect("an answer"), "5433");
    assert!(printed.contains("`no` is not a port"), "{printed}");
    assert!(printed.contains("`nope` is not a port"), "{printed}");
}

#[test]
fn a_checked_question_re_asks_on_a_refused_value_rather_than_writing_it() {
    // The rule that makes this a loop: a value that fails the check is printed and
    // the same question is asked again, so the refused value never leaves.
    let (answer, printed) = run_checked("x\n7\n", |out, reader| {
        ask_checked(out, reader, Palette::new(false), "Port", None, |value| {
            if value == "7" {
                Ok(())
            } else {
                Err(format!("`{value}` is not the port"))
            }
        })
    });
    assert_eq!(answer.expect("an answer"), "7");
    assert_eq!(
        printed.matches("Port").count(),
        2,
        "the question is asked once per attempt: {printed}"
    );
}

#[test]
fn a_checked_question_stops_when_input_ends() {
    // A reader that runs out is a usage error naming the flag, not a value. A test
    // that scripted too few lines has to fail as itself, not as a passed check.
    let (answer, _printed) = run_checked("", |out, reader| {
        ask_checked(out, reader, Palette::new(false), "Port", None, |_| Ok(()))
    });
    assert!(matches!(answer, Err(Signal::Usage(_))), "{answer:?}");
}

#[test]
fn a_checked_field_reads_a_reserved_word_as_text() {
    // The same consequence as the free-text case, on the re-asking path: a
    // validator that accepted every value must still return `b` as the answer.
    let (answer, _printed) = run_checked("b\n", |out, reader| {
        ask_checked(out, reader, Palette::new(false), "Port", None, |_| Ok(()))
    });
    assert_eq!(answer.expect("an answer"), "b");
}

#[test]
fn the_back_hint_names_the_keys_that_actually_work() {
    assert_eq!(BACK_HINT, "Esc goes back · Enter keeps going");
}

#[test]
fn a_refusal_carries_its_text_and_the_dialog_signals_carry_none() {
    let refusal = Signal::Refused("no".to_owned());
    assert_eq!(refusal.reason(), "no");
    assert_eq!(Signal::Back.reason(), "");
    assert_eq!(Signal::Quit.reason(), "");
}

#[test]
fn a_usage_stop_and_a_refusal_are_built_by_one_constructor() {
    // The two leave the process with different exit codes, so a caller that has to
    // remember which is which would eventually get it wrong.
    assert_eq!(
        Signal::usage("bad flag").reason(),
        Signal::Usage("bad flag".to_owned()).reason()
    );
}
