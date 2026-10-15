// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the reserved surfaces' argument contract against the real
//! `clio` binary: the documented global flags in both positions, the refusal of
//! an unknown flag and of a stray positional, the help surface's `--markdown`
//! request, and that a usage error keeps the caller's requested output mode.
//!
//! ## Owns
//!
//! - End-to-end assertions over spawned `clio` processes for the reserved
//!   surfaces (`status`, `mcp schema-export`, and `help --markdown`)
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
use std::sync::atomic::{AtomicU64, Ordering};

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

/// Calls served so far in this test binary, so a scratch directory never
/// repeats: the process id alone is constant, and two scenarios must not share
/// one manual directory.
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Unique scratch directory for one scenario, the pid-nanos-counter pattern the
/// sibling harnesses use.
/// Unique scratch directory for one case, removed when the guard drops.
fn temp_dir(tag: &str) -> common::Scratch {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    // Canonicalize the temp root before joining: the writer refuses every
    // symlink path component, and on macOS the system temp directory sits
    // behind symlinks (`/var` and `/tmp` point into `/private`), which would
    // fail the successful writes below for an environmental reason.
    let base = std::fs::canonicalize(std::env::temp_dir()).expect("the temp root resolves");
    let dir = base.join(format!(
        "clio-reserved-args-{tag}-{}-{nanos}-{sequence}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    common::Scratch::adopt(dir)
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

/// `clio help --markdown` without `--out` exits 2 at the process boundary: the
/// missing output directory is a usage refusal, not a partial write.
#[test]
fn a_markdown_request_without_out_exits_two() {
    let output = run(&["help", "--markdown"]);
    let err = stderr_of(&output);
    assert_eq!(output.status.code(), Some(2), "{err}");
    assert!(err.contains("--markdown"), "{err}");
}

/// The complete manual request writes the directory and exits 0: the success the
/// in-process tests cannot pin at the process boundary.
#[test]
fn a_markdown_request_writes_the_manual_and_exits_zero() {
    let out = temp_dir("manual");
    let out_arg = out.to_str().expect("utf-8 scratch path");
    let output = run(&["help", "--markdown", "--out", out_arg]);
    let err = stderr_of(&output);
    assert_eq!(output.status.code(), Some(0), "{err}");
    assert!(out.join("index.md").is_file(), "the index page: {err}");
    assert!(out.join("help.md").is_file(), "the help leaf page: {err}");
}

/// A leading global's value is not the TOPIC the manual request refuses:
/// `clio --db PATH help --markdown --out DIR` writes the manual and exits 0,
/// even though `main` lifted `--db PATH` and appended its value after the verb.
#[test]
fn a_leading_global_value_is_not_the_markdown_topic() {
    let out = temp_dir("manual-global");
    let db = out.join("led.db");
    let manual = out.join("manual");
    let output = run(&[
        "--db",
        db.to_str().expect("utf-8 db path"),
        "help",
        "--markdown",
        "--out",
        manual.to_str().expect("utf-8 manual path"),
    ]);
    let err = stderr_of(&output);
    assert_eq!(output.status.code(), Some(0), "{err}");
    assert!(manual.join("index.md").is_file(), "the index page: {err}");
}
