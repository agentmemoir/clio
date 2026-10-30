// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `clio ops` output-mode contract: how `--output text`
//! routes verify and diagnose through the human renderer, how `--output json`
//! keeps the one-line JSON payload, what the piped default prints, and how an
//! invalid mode fails closed.
//!
//! ## Owns
//! - The `--output text` and `--output json` mode tests for `verify` and
//!   `diagnose`
//! - The piped-default JSON-line test
//! - The invalid `--output` usage-error test
//! - The captured-output helper these tests run through
//!
//! ## Does not own
//! - The renderers the text mode reaches (`ops_cli_render`)
//! - The field-coverage assertions for the render functions
//!   (`ops_cli_render_tests`)
//! - The exit-code, flag, and key-store tests (`ops_cli_tests`)
//!
//! ## Boundary
//!
//! Tests here break when the `--output` mode contract changes; they never
//! change it.

use super::tests::{memory, no_env};
use super::*;

/// The `--output text` seam: run `ops` and capture what the report emitter
/// wrote, so a test reads the payload instead of racing the real stdout.
fn run_ops_captured(args: Vec<String>) -> (ExitCode, String) {
    let mut out: Vec<u8> = Vec::new();
    let code = ops_with_out(args, &no_env, &mut out);
    let text = String::from_utf8(out).expect("ops output is utf8");
    (code, text)
}

/// `--output text` routes verify through the human renderer: no JSON braces,
/// the verdict and scope lines the renderer writes.
#[test]
fn verify_output_text_renders_human_text() {
    let mut args = memory("verify");
    args.extend(["--output", "text"].map(str::to_owned));
    let (code, text) = run_ops_captured(args);
    assert_eq!(code, ExitCode::SUCCESS);
    assert!(!text.contains('{') && !text.contains('}'), "{text}");
    assert!(text.contains("verify: ok"), "{text}");
    assert!(text.contains("scope: store"), "{text}");
    assert!(text.contains("findings:"), "{text}");
}

/// `--output json` keeps the one-line JSON payload verify has always printed.
#[test]
fn verify_output_json_keeps_the_json_line() {
    let mut args = memory("verify");
    args.extend(["--output", "json"].map(str::to_owned));
    let (code, text) = run_ops_captured(args);
    assert_eq!(code, ExitCode::SUCCESS);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 1, "{text}");
    assert!(lines[0].starts_with("{\"ok\":true"), "{text}");
    assert!(lines[0].contains("\"scope\":\"store\""), "{text}");
}

/// `diagnose` follows the same mode contract as verify.
#[test]
fn diagnose_output_text_renders_human_text() {
    let mut args = memory("diagnose");
    args.extend(["--output", "text"].map(str::to_owned));
    let (code, text) = run_ops_captured(args);
    assert_eq!(code, ExitCode::SUCCESS);
    assert!(!text.contains('{') && !text.contains('}'), "{text}");
    assert!(text.contains("diagnose: ok"), "{text}");
    assert!(text.contains("deeper checks: clio ops verify"), "{text}");
}

/// Without `--output`, the piped default stays the JSON line the command
/// always printed.
#[test]
fn verify_default_output_stays_json() {
    let (code, text) = run_ops_captured(memory("verify"));
    assert_eq!(code, ExitCode::SUCCESS);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 1, "{text}");
    assert!(lines[0].starts_with("{\"ok\":"), "{text}");
}

/// An invalid `--output` value behaves like every other command surface: a
/// usage error on stderr with the shared usage exit code, and nothing on the
/// data channel.
#[test]
fn invalid_output_value_reports_a_usage_error_exits_2() {
    let mut args = memory("verify");
    args.extend(["--output", "bogus"].map(str::to_owned));
    let (code, text) = run_ops_captured(args);
    assert_eq!(code, ExitCode::from(2));
    assert!(text.is_empty(), "usage errors go to stderr, got: {text}");
}
