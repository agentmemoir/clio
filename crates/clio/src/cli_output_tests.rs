// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the shared CLI output contract: mode selection, JSON/text
//! emission, color rules, and error rendering.
//!
//! ## Owns
//! - Assertions over `OutputMode`, `write_*`, and color helpers
//!
//! ## Does not own
//! - The output implementation under test (`cli_output`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;
use crate::cli_error::CliError;
use serde_json::json;

fn tokens(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

#[test]
fn explicit_output_beats_tty_detection() {
    assert_eq!(
        resolve_output(Some("json"), true).expect("json"),
        OutputMode::Json
    );
    assert_eq!(
        resolve_output(Some("text"), false).expect("text"),
        OutputMode::Text
    );
}

#[test]
fn default_output_follows_tty() {
    assert_eq!(resolve_output(None, true).expect("tty"), OutputMode::Text);
    assert_eq!(resolve_output(None, false).expect("pipe"), OutputMode::Json);
}

#[test]
fn invalid_output_value_is_a_usage_error() {
    let error = resolve_output(Some("yaml"), false).expect_err("invalid");
    assert_eq!(error.code(), "usage");
    assert!(error.hint().is_some());
}

#[test]
fn mode_from_raw_honors_an_explicit_output() {
    assert_eq!(
        mode_from_raw(&tokens(&["q", "--output", "text", "--limit", "2"]), false),
        OutputMode::Text
    );
    assert_eq!(
        mode_from_raw(&tokens(&["q", "--output", "json"]), true),
        OutputMode::Json
    );
    assert_eq!(
        mode_from_raw(&tokens(&["--output=json"]), true),
        OutputMode::Json
    );
    // The last explicit value wins, matching the parser's overwrite rule.
    assert_eq!(
        mode_from_raw(&tokens(&["--output", "json", "--output", "text"]), false),
        OutputMode::Text
    );
}

#[test]
fn mode_from_raw_falls_back_when_output_is_absent_or_unusable() {
    assert_eq!(mode_from_raw(&tokens(&[]), true), OutputMode::Text);
    assert_eq!(mode_from_raw(&tokens(&[]), false), OutputMode::Json);
    assert_eq!(
        mode_from_raw(&tokens(&["--output"]), false),
        OutputMode::Json
    );
    assert_eq!(
        mode_from_raw(&tokens(&["--output", "--db", "x"]), false),
        OutputMode::Json
    );
    assert_eq!(
        mode_from_raw(&tokens(&["--output", "yaml"]), false),
        OutputMode::Json
    );
    assert_eq!(
        mode_from_raw(&tokens(&["--outputx"]), true),
        OutputMode::Text
    );
}

#[test]
fn mode_from_raw_honors_the_short_output_alias() {
    assert_eq!(
        mode_from_raw(&tokens(&["q", "-o", "text", "--limit", "2"]), false),
        OutputMode::Text
    );
    assert_eq!(mode_from_raw(&tokens(&["-o=json"]), true), OutputMode::Json);
    // The last explicit spelling wins across both spellings, matching the
    // parser's overwrite rule.
    assert_eq!(
        mode_from_raw(&tokens(&["--output", "json", "-o", "text"]), false),
        OutputMode::Text
    );
    assert_eq!(
        mode_from_raw(&tokens(&["-o", "text", "--output=json"]), true),
        OutputMode::Json
    );
}

#[test]
fn mode_from_raw_falls_back_for_unusable_short_output_forms() {
    // A bare `-o`, one followed by a flag, an unknown value, and an attached
    // token all fall back to the terminal default like their `--output` peers.
    assert_eq!(mode_from_raw(&tokens(&["-o"]), false), OutputMode::Json);
    assert_eq!(
        mode_from_raw(&tokens(&["-o", "--db", "x"]), false),
        OutputMode::Json
    );
    assert_eq!(
        mode_from_raw(&tokens(&["-o=yaml"]), false),
        OutputMode::Json
    );
    assert_eq!(mode_from_raw(&tokens(&["-otext"]), true), OutputMode::Text);
}

#[test]
fn color_requires_a_terminal_and_no_no_color() {
    assert!(supports_color(true, false));
    assert!(!supports_color(true, true));
    assert!(!supports_color(false, false));
}

#[test]
fn write_json_is_one_compact_line() {
    let mut out = Vec::new();
    write_json(&mut out, &json!({"ok": true, "n": 1})).expect("write");
    let text = String::from_utf8(out).expect("utf8");
    assert_eq!(text.lines().count(), 1);
    let parsed: serde_json::Value = serde_json::from_str(&text).expect("json");
    assert_eq!(parsed, json!({"ok": true, "n": 1}));
}

#[test]
fn write_text_appends_a_newline() {
    let mut out = Vec::new();
    write_text(&mut out, "hello").expect("write");
    assert_eq!(String::from_utf8(out).expect("utf8"), "hello\n");
}

#[test]
fn write_error_renders_envelope_for_json_mode() {
    let mut out = Vec::new();
    let error = CliError::usage("bad flag").with_hint("try --limit");
    write_error(&mut out, &error, OutputMode::Json, false).expect("write");
    let text = String::from_utf8(out).expect("utf8");
    assert!(text.contains("\"ok\":false"));
    assert!(text.contains("\"hint\":\"try --limit\""));
    assert!(!text.contains('\u{1b}'));
}

#[test]
fn write_error_renders_plain_text_without_color() {
    let mut out = Vec::new();
    write_error(&mut out, &CliError::usage("bad"), OutputMode::Text, false).expect("write");
    assert_eq!(String::from_utf8(out).expect("utf8"), "error: bad\n");
}

#[test]
fn write_error_colors_text_on_a_terminal() {
    let mut out = Vec::new();
    write_error(&mut out, &CliError::usage("bad"), OutputMode::Text, true).expect("write");
    let text = String::from_utf8(out).expect("utf8");
    assert!(text.starts_with('\u{1b}'));
    assert!(text.contains("error: bad"));
}
