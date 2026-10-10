// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the reserved surfaces' argument contract against the real
//! `clio` binary: the documented global flags in both positions, the refusal of
//! an unknown flag and of a stray positional, and that a usage error keeps the
//! caller's requested output mode.
//!
//! ## Owns
//!
//! - End-to-end assertions over spawned `clio` processes for the reserved
//!   surfaces (`status`, `mcp schema-export`)
//!
//! ## Does not own
//!
//! - The in-process parser tests (`cli_top_level_tests`)
//! - The CLI implementation under test
//!
//! ## Boundary
//!
//! Tests here break when the reserved surfaces' argument contract changes; they
//! never change it.
//!
//! Keep this module focused on the reserved surfaces' process contract.

mod common;

use std::process::{Command, Output, Stdio};

/// The `clio` binary with a cleared environment plus the coverage pattern.
fn clio() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_clio"));
    cmd.env_clear();
    // Forward the coverage profile pattern so instrumented children write into
    // the gitignored target directory instead of the source tree.
    if let Some(pattern) = std::env::var_os("LLVM_PROFILE_FILE") {
        cmd.env("LLVM_PROFILE_FILE", pattern);
    }
    common::seed_key(&mut cmd);
    cmd
}

/// Run one command with piped streams and no appended arguments.
fn run(args: &[&str]) -> Output {
    clio()
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn clio")
}

fn stderr_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// A documented global flag before the verb must reach the surface, not be
/// refused: `split_leading_globals` hands it back after the verb's own arguments.
#[test]
fn a_global_flag_before_the_verb_is_accepted_on_status() {
    let output = run(&["--actor", "bob", "status", "--db", "sqlite::memory:"]);
    let err = stderr_of(&output);
    assert!(
        !err.contains("unknown flag `--actor`"),
        "a documented global must be accepted before the verb: {err}"
    );
    assert_ne!(output.status.code(), Some(2), "{err}");
}

/// The same global after the verb is the plain form and must also be accepted.
#[test]
fn a_global_flag_after_the_verb_is_accepted_on_status() {
    let output = run(&["status", "--actor", "bob", "--db", "sqlite::memory:"]);
    let err = stderr_of(&output);
    assert!(
        !err.contains("unknown flag `--actor`"),
        "a documented global must be accepted after the verb: {err}"
    );
    assert_ne!(output.status.code(), Some(2), "{err}");
}

/// A token with no leading dash is refused rather than dropped: these surfaces
/// have no positional operand, so a mistyped flag cannot become a silent no-op.
#[test]
fn a_stray_positional_is_a_usage_error() {
    let output = run(&["status", "--db", "sqlite::memory:", "stray"]);
    let err = stderr_of(&output);
    assert_eq!(output.status.code(), Some(2), "{err}");
    assert!(err.contains("unexpected argument `stray`"), "{err}");
}

/// A usage error keeps the output mode the caller asked for, which is the mode
/// resolved from the same tokens the surface parsed.
#[test]
fn a_usage_error_honours_the_requested_output_mode() {
    let text = run(&["mcp", "schema-export", "--output", "text", "--bogus"]);
    assert_eq!(text.status.code(), Some(2), "{}", stderr_of(&text));
    let text_err = stderr_of(&text);
    assert!(
        !text_err.trim_start().starts_with('{'),
        "the text mode must not emit the JSON envelope: {text_err}"
    );

    let json = run(&["mcp", "schema-export", "--output", "json", "--bogus"]);
    assert_eq!(json.status.code(), Some(2), "{}", stderr_of(&json));
    let json_err = stderr_of(&json);
    assert!(
        json_err.trim_start().starts_with('{'),
        "the json mode must emit the envelope: {json_err}"
    );
}

/// A bare `--` is the top level's delimiter before positional text, and these
/// surfaces take no operand, so it is accepted as the last token.
#[test]
fn a_bare_double_dash_is_accepted() {
    let output = run(&["status", "--db", "sqlite::memory:", "--"]);
    let err = stderr_of(&output);
    assert_eq!(output.status.code(), Some(0), "{err}");
}
