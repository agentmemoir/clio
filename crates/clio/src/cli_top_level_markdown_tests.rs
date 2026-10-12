// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the help record's whole-manual request: the accepted
//! `--markdown --out` spelling, the exclusive output modes, the missing output
//! path, the global values `main` appends, and the repeated and blank refusals.
//!
//! ## Owns
//! - Assertions over `markdown_request` and `wants_json_catalog`
//!
//! ## Does not own
//! - The parsing under test (the `cli_top_level` module)
//! - The writer the request reaches (see `cli_manual_write_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the manual request's validation.

use super::*;
use crate::cli_error::CliError;
use std::process::ExitCode;

/// The arguments as the dispatch receives them.
fn args(words: &[&str]) -> Vec<String> {
    words.iter().map(|word| String::from(*word)).collect()
}

/// One refusal: the usage machine code and the exit code it maps to.
fn assert_usage_refusal(error: &CliError, context: &str) {
    assert_eq!(error.code(), "usage", "{context}");
    assert_eq!(error.exit_code(), ExitCode::from(2), "{context}");
}

#[test]
fn a_markdown_and_out_request_names_the_directory() {
    assert_eq!(
        markdown_request(&args(&["--markdown", "--out", "./manual"]))
            .expect("a complete manual request"),
        Some(PathBuf::from("./manual"))
    );
}

#[test]
fn an_equals_spelling_names_the_directory_too() {
    assert_eq!(
        markdown_request(&args(&["--markdown", "--out=./manual"]))
            .expect("a complete manual request"),
        Some(PathBuf::from("./manual"))
    );
}

#[test]
fn arguments_without_markdown_ask_for_no_manual() {
    assert_eq!(markdown_request(&args(&[])).expect("plain help"), None);
    assert_eq!(
        markdown_request(&args(&["--json"])).expect("the JSON catalog"),
        None
    );
    assert_eq!(markdown_request(&args(&["backup"])).expect("a topic"), None);
}

/// `main` lifts the leading globals and appends their value tokens after the
/// verb, so both `clio --db PATH help --markdown --out DIR` and the trailing
/// order arrive as the arguments read here; the value of a global value flag
/// must never be classified as a TOPIC, or the manual write is refused.
#[test]
fn the_value_of_a_global_value_flag_is_not_a_topic() {
    for words in [
        vec!["--markdown", "--out", "manual", "--db", "/tmp/clio.db"],
        vec!["--markdown", "--out", "manual", "--actor", "bob"],
        vec!["--markdown", "--out", "manual", "-o", "json"],
    ] {
        assert_eq!(
            markdown_request(&args(&words)).expect("a global value is not a topic"),
            Some(PathBuf::from("manual")),
            "args {words:?}"
        );
    }
}

/// Only a value token is skipped: a real topic after a global's value is still
/// the TOPIC `--markdown` refuses.
#[test]
fn a_topic_after_a_global_value_is_still_refused() {
    let error = markdown_request(&args(&[
        "--markdown",
        "--out",
        "manual",
        "--db",
        "/tmp/clio.db",
        "backup",
    ]))
    .expect_err("a real topic after the global value");
    assert_usage_refusal(&error, "the global value does not swallow the topic");
}

#[test]
fn markdown_without_out_is_refused() {
    let error = markdown_request(&args(&["--markdown"])).expect_err("missing --out");
    assert_usage_refusal(&error, "missing --out");
}

#[test]
fn out_without_markdown_is_refused() {
    let error = markdown_request(&args(&["--out", "manual"])).expect_err("--out alone");
    assert_usage_refusal(&error, "--out alone");
}

#[test]
fn markdown_and_json_are_exclusive_in_either_order() {
    for words in [
        vec!["--markdown", "--json", "--out", "manual"],
        vec!["--json", "--markdown", "--out", "manual"],
    ] {
        let error = markdown_request(&args(&words)).expect_err("exclusive modes");
        assert_usage_refusal(&error, &format!("args {words:?}"));
    }
}

#[test]
fn markdown_takes_no_topic() {
    let error =
        markdown_request(&args(&["--markdown", "--out", "manual", "backup"])).expect_err("topic");
    assert_usage_refusal(&error, "a topic");
}

#[test]
fn either_new_flag_is_refused_twice() {
    for words in [
        vec!["--markdown", "--markdown", "--out", "manual"],
        vec!["--markdown", "--out", "a", "--out", "b"],
    ] {
        let error = markdown_request(&args(&words)).expect_err("repeated flag");
        assert_usage_refusal(&error, &format!("args {words:?}"));
    }
}

#[test]
fn out_refuses_a_missing_or_blank_value() {
    for words in [
        vec!["--markdown", "--out"],
        vec!["--markdown", "--out", ""],
        vec!["--markdown", "--out", "--json"],
    ] {
        let error = markdown_request(&args(&words)).expect_err("no directory value");
        assert_usage_refusal(&error, &format!("args {words:?}"));
    }
}

/// A space-form value that starts with a dash is a missing value, never a
/// directory named after the flag that follows; the explicit `--out=` spelling
/// keeps its token as typed, dashes included.
#[test]
fn a_space_form_out_value_cannot_start_with_a_dash() {
    let error = markdown_request(&args(&["--markdown", "--out", "-h"])).expect_err("no value");
    assert_usage_refusal(&error, "--out -h is a missing value");
    assert_eq!(
        markdown_request(&args(&["--markdown", "--out=-h"])).expect("an explicit value"),
        Some(PathBuf::from("-h"))
    );
}

#[test]
fn only_the_json_flag_selects_the_catalog() {
    assert!(wants_json_catalog(&args(&["--json"])));
    assert!(!wants_json_catalog(&args(&["--markdown"])));
    assert!(!wants_json_catalog(&args(&[])));
}
