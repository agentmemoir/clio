// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the value rules of the CLI argument grammar: a value flag
//! given twice is refused, a blank value or positional is refused, and a
//! timestamp flag accepts only a timestamp.
//!
//! ## Owns
//! - Assertions over `parse` for repeated, blank, and timestamp values
//!
//! ## Does not own
//! - The parser implementation under test (`cli_args`)
//! - Flag shape, delimiter, and leading-global behaviour (see `cli_args_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;

fn spec() -> FlagSpec {
    FlagSpec::new(&["limit", "domains"], &["snapshot", "gist"])
}

fn tokens(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

#[test]
fn a_repeated_value_flag_is_refused_by_name() {
    let error = parse(&spec(), tokens(&["--limit=1", "--limit", "2"])).expect_err("repeat");
    assert_eq!(error.code(), "usage");
    assert!(
        error.message().contains("--limit was given more than once"),
        "{}",
        error.message()
    );
}

#[test]
fn a_repeated_short_output_flag_is_refused() {
    let error = parse(&spec(), tokens(&["-o", "json", "-o", "text"])).expect_err("repeat");
    assert!(
        error.message().contains("-o was given more than once"),
        "{}",
        error.message()
    );
}

#[test]
fn a_whitespace_only_value_is_refused_by_name() {
    let error = parse(&spec(), tokens(&["--limit", "   "])).expect_err("blank value");
    assert_eq!(error.code(), "usage");
    assert!(
        error
            .message()
            .contains("--limit must not be empty or whitespace-only"),
        "{}",
        error.message()
    );
}

#[test]
fn a_blank_positional_is_refused_before_a_verb_can_store_it() {
    let error = parse(&spec(), tokens(&["   "])).expect_err("blank positional");
    assert_eq!(error.code(), "usage");
    assert!(
        error
            .message()
            .contains("positional argument must not be empty"),
        "{}",
        error.message()
    );
}

#[test]
fn a_blank_positional_after_the_delimiter_is_refused() {
    let error = parse(&spec(), tokens(&["--", ""])).expect_err("blank positional");
    assert!(
        error
            .message()
            .contains("positional argument must not be empty"),
        "{}",
        error.message()
    );
}

#[test]
fn a_timestamp_flag_accepts_each_form_the_store_reads() {
    let timed = FlagSpec::new(&["as-of"], &[]);
    for value in [
        "2026-01-01T00:00:00Z",
        "2026-01-01T00:00:00+05:30",
        "2026-01-01",
        "2026-01-01T00:00:00",
    ] {
        let parsed = parse(&timed, tokens(&["--as-of", value])).expect(value);
        assert_eq!(parsed.value("as-of"), Some(value));
    }
}

#[test]
fn a_timestamp_flag_refuses_a_value_that_is_not_a_timestamp() {
    let timed = FlagSpec::new(&["valid-from"], &[]);
    let error = parse(&timed, tokens(&["--valid-from", "bogus"])).expect_err("bad timestamp");
    assert_eq!(error.code(), "usage");
    assert!(
        error
            .message()
            .contains("--valid-from must be a timestamp such as 2026-01-01T00:00:00Z"),
        "{}",
        error.message()
    );
}

#[test]
fn a_flag_that_is_not_a_timestamp_takes_any_value() {
    let parsed = parse(&spec(), tokens(&["--limit", "bogus"])).expect("parse");
    assert_eq!(parsed.value("limit"), Some("bogus"));
}
