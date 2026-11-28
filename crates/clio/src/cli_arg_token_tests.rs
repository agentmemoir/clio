// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the one token rule both CLI parsers share: which raw
//! tokens are flags rather than values, how an inline `--flag=value` token is
//! read, and the two refusals that follow from the rule.
//!
//! ## Owns
//! - Assertions over `is_flag_token` for the dash-led, lone-dash and plain forms
//! - Assertions over `out_value` and `unexpected_argument`
//!
//! ## Does not own
//! - What a parser then does with the token (`cli_args_tests`,
//!   `cli_args_value_tests`, `cli_top_level_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;

fn tokens(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

/// Every dash-led token is a flag token, whether it is a short spelling, a long
/// one, or the whole of an inline `--flag=value`.
#[test]
fn a_dash_led_token_is_a_flag_token() {
    for token in [
        "-x",
        "-o",
        "-h",
        "--x",
        "--db",
        "--db=x.db",
        "--db=-x",
        "--file",
    ] {
        assert!(is_flag_token(token), "`{token}` is a flag token");
    }
}

/// The lone `-` is the one dash that is a value, so a value flag may take it:
/// `--file -` means stdin, and `--out -` is refused for the separate reason that
/// `--out` names a directory.
#[test]
fn a_lone_dash_is_a_value_and_not_a_flag_token() {
    assert!(!is_flag_token("-"));
}

/// A token with no dash is a word or a value, never a flag token.
#[test]
fn a_token_without_a_dash_is_not_a_flag_token() {
    for token in ["json", "x", "", "x-y", "path/to/x.db", "stdin"] {
        assert!(!is_flag_token(token), "`{token}` is not a flag token");
    }
}

/// An inline `--flag=value` is one dash-led token, so no consumer ever sees its
/// value as a separate token: the dash-led value travels inside the flag token,
/// which is why `--db=-x` still names the database `-x`.
#[test]
fn an_inline_value_is_one_dash_led_token() {
    assert!(is_flag_token("--db=-x"));
    // The value alone would be refused as a value; inside the token it is kept,
    // so the whole-token answer is the one that matters.
    assert!(is_flag_token("-x"));
    // An inline form whose value has no dash is a flag token too, and a plain
    // `--flag=` keeps the empty value on the flag it belongs to.
    assert!(is_flag_token("--db=x"));
    assert!(is_flag_token("--out="));
}

/// `--out` takes a plain directory and refuses an absent, dash-led, or blank
/// token, so a value flag never swallows the flag that follows it.
#[test]
fn out_takes_a_plain_value_and_refuses_anything_dash_led() {
    let args = tokens(&["--out", "docs"]);
    assert_eq!(out_value(&args, 0).expect("a plain value"), "docs");

    for argv in [
        vec!["--out"],
        vec!["--out", "-h"],
        vec!["--out", "--markdown"],
        vec!["--out", "-"],
    ] {
        let error = out_value(&tokens(&argv), 0).expect_err("no directory value");
        assert_eq!(error.code(), "usage", "{argv:?}");
        assert_eq!(
            error.message(),
            "--out requires a directory value",
            "{argv:?}"
        );
    }

    let blank = out_value(&tokens(&["--out", "   "]), 0).expect_err("blank directory");
    assert!(
        blank
            .message()
            .contains("--out must not be empty or whitespace-only"),
        "{}",
        blank.message()
    );
}

/// A token with no dash has no operand slot on a reserved surface, so it is
/// refused as typed rather than dropped.
#[test]
fn an_operand_less_surface_refuses_a_token_without_a_dash() {
    let error = unexpected_argument("stray");
    assert_eq!(error.code(), "usage");
    assert_eq!(error.message(), "unexpected argument `stray`");
    assert_eq!(
        error.hint(),
        Some("these commands take flags only; run `clio help` for the list")
    );
}
