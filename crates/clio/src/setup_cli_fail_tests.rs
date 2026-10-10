// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for usage and failure-mode tests of the `clio
//! setup` command surface: unknown and missing flags, the `--type` retirement,
//! contradictory flags caught before a prompt, non-interactive fast failures,
//! and prompt I/O that fails.
//!
//! ## Owns
//! - Negative-path behavior under test for `setup_cli`
//!
//! ## Does not own
//! - Production CLI logic under test
//! - Happy-path command-surface tests (see `setup_cli_tests`)
//!
//! ## Boundary
//!
//! Changes to setup error messages or exit codes should normally update tests
//! here. Changes to happy-path behavior should not require changes here.
//!
//! Keep this module focused on setup usage errors.

use super::*;
use crate::setup_support_tests::{StubDocker, invoke_setup};
use std::io::{self, BufRead};

/// One `clio setup` run over injected streams and a Docker stub.
///
/// `--config` is added unless the caller supplied one, pointing at a path that
/// does not exist, so a usage test never reads the config on the machine running
/// the suite and turns a missing-flag refusal into a database question.
fn invoke(args: &[&str], input: &str, is_tty: bool) -> (ExitCode, String, String) {
    let mut full: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
    if !full.iter().any(|arg| arg == "--config") {
        full.push("--config".to_owned());
        full.push(crate::setup_preview_fixtures::ABSENT_CONFIG.to_owned());
    }
    let config = PathBuf::from(crate::setup_preview_fixtures::ABSENT_CONFIG);
    let run = invoke_setup(
        &full,
        input,
        is_tty,
        &config,
        &StubDocker::ok(),
        &crate::setup_test_seam::Probes::ok(),
    );
    (run.code, run.out, run.err)
}

fn expect_usage(args: &[&str], needle: &str) {
    let (code, out, err) = invoke(args, "", false);
    assert_eq!(code, ExitCode::from(2), "expected usage error, err: {err}");
    assert!(
        err.contains(needle),
        "missing {needle:?} in {err:?} (out: {out})"
    );
}

/// The three service-role flags that let a run reach the advanced settings, so a
/// refusal about those settings is the one that surfaces.
const SERVICES: [&str; 6] = ["--embed", "local", "--rerank", "skip", "--extract", "skip"];

fn with_services<'a>(args: &[&'a str]) -> Vec<&'a str> {
    let mut all: Vec<&str> = args.to_vec();
    all.extend(SERVICES);
    all
}

#[test]
fn argument_parsing_errors_exit_two() {
    expect_usage(&["--nope"], "unknown flag `--nope`");
    expect_usage(&["stray"], "unexpected argument `stray`");
    expect_usage(&["--database"], "--database requires a value");
    expect_usage(&["--database", "--yes"], "--database requires a value");
}

#[test]
fn the_retired_type_flag_names_its_replacement() {
    let (code, _out, err) = invoke(&["--type", "custom"], "", false);
    assert_eq!(code, ExitCode::from(2));
    assert!(err.contains("--type was removed"), "{err}");
    assert!(err.contains("--database"), "{err}");
    assert!(err.contains("--embed"), "{err}");
}

#[test]
fn an_unknown_database_choice_is_refused() {
    expect_usage(
        &["--database", "sideways"],
        "--database must be managed|sqlite|existing",
    );
}

#[test]
fn an_unknown_hand_over_is_refused() {
    expect_usage(
        &["--database", "existing", "--database-connect", "sideways"],
        "--database-connect must be pasted|typed",
    );
}

#[test]
fn a_contradictory_flag_set_fails_before_any_question() {
    expect_usage(
        &["--embed", "local", "--embed-dims", "1536"],
        "--embed-dims applies only to a hosted embed service",
    );
    expect_usage(
        &["--rerank", "local", "--rerank-key", "k"],
        "--rerank-key cannot be given with a local rerank",
    );
    expect_usage(
        &["--embed-dims", "0"],
        "--embed-dims must be a whole number above zero",
    );
    expect_usage(
        &["--embed-dims", "many"],
        "--embed-dims must be a whole number",
    );
}

#[test]
fn a_contradictory_set_shows_no_prompt_at_all() {
    let (_, out, _err) = invoke(
        &["--embed", "local", "--embed-dims", "1536"],
        "1\n1\n1\n1\n",
        true,
    );
    assert!(!out.contains("Where should"), "{out}");
}

#[test]
fn a_hosted_service_without_a_key_fails_fast() {
    expect_usage(
        &[
            "--database",
            "sqlite",
            "--embed",
            "online",
            "--embed-url",
            "https://embed.example.test",
            "--embed-model",
            "m",
        ],
        "--embed-key is required without a terminal",
    );
}

#[test]
fn a_service_role_is_required_without_a_terminal() {
    expect_usage(
        &["--database", "sqlite", "--rerank", "skip"],
        "--embed local|online is required without a terminal",
    );
}

/// A forced non-interactive run with no flags fails fast and never prompts.
///
/// It is asked through the core rather than through `main::run` so the
/// environment is injected. A `CLIO_DATABASE_URL` exported in the developer's shell
/// is a legitimate answer to "which database", so with one present the run
/// reaches the connection test and reports that instead of a usage error. Both
/// exit without prompting, but only the usage error is what this test is about,
/// and only the injected environment makes that true on every machine.
#[test]
fn a_non_interactive_setup_with_no_flags_is_a_usage_error() {
    expect_usage(&["--non-interactive"], "required without a terminal");
}

#[test]
fn an_existing_paste_route_needs_an_address_without_a_terminal() {
    expect_usage(
        &["--database", "existing", "--database-connect", "pasted"],
        "--database-url is required without a terminal",
    );
}

#[test]
fn an_existing_typed_route_needs_its_fields_without_a_terminal() {
    expect_usage(
        &["--database", "existing", "--database-connect", "typed"],
        "--database-host is required without a terminal",
    );
}

#[test]
fn a_remote_plain_http_endpoint_is_refused() {
    expect_usage(
        &[
            "--database",
            "sqlite",
            "--embed",
            "online",
            "--embed-url",
            "http://embed.example.test",
            "--embed-model",
            "m",
            "--embed-key",
            "k",
        ],
        "must use https for a remote host",
    );
}

/// A fully flagged typed route, which is the only route that asks for the
/// advanced settings.
fn typed_route() -> Vec<&'static str> {
    [
        "--database",
        "existing",
        "--database-connect",
        "typed",
        "--database-host",
        "db.example.com",
        "--database-port",
        "5432",
        "--database-name",
        "clio",
        "--database-user",
        "clio",
        "--database-password",
        "pw",
        "--database-ssl",
        "prefer",
        "--embed",
        "local",
        "--rerank",
        "skip",
        "--extract",
        "skip",
    ]
    .to_vec()
}

fn with_option(setting: &str) -> Vec<&str> {
    let mut args = typed_route();
    args.extend(["--database-option", setting]);
    args
}

#[test]
fn an_unknown_advanced_setting_is_refused() {
    expect_usage(
        &with_option("sideways=1"),
        "`sideways` is not an advanced connection setting",
    );
    expect_usage(
        &with_option("keepalives"),
        "--database-option needs name=value",
    );
}

#[test]
fn the_advanced_settings_are_refused_on_a_route_that_never_asks_for_them() {
    // Silently ignoring the flag would report a success and leave the setting out
    // of the address, which is worse than refusing.
    expect_usage(
        &with_services(&[
            "--database",
            "sqlite",
            "--database-option",
            "options=-c%20k1",
        ]),
        "--database-option applies to",
    );
}

#[test]
fn an_interactive_flag_without_a_terminal_fails() {
    expect_usage(&["--interactive"], "--interactive requires a terminal");
}

#[test]
fn a_prompt_with_no_input_names_the_equivalent_flag() {
    // One answer, then input ends. Which question that answer reaches does not
    // matter — the property is that the stop names the question it ran out at and
    // the flag that would have answered it, so the assertion is on that shape
    // rather than on one question that a route change could move.
    let (code, _out, err) = invoke(&["--interactive", "--key-store", "file"], "1\n", true);
    assert_eq!(code, ExitCode::from(2), "err: {err}");
    assert!(
        err.contains("no input for `"),
        "the message names the question that ran out: {err}"
    );
    assert!(err.contains("pass the equivalent flag"), "{err}");
}

#[test]
fn a_question_with_no_default_cannot_be_left_empty() {
    // The typed route's host is the one field the plan gives no default to on a
    // fresh install, because a first run cannot invent a machine to write to. An
    // empty line there is a refusal rather than an answer, so the script takes the
    // existing route (`3`), asks to type the details (`2`), and leaves the host
    // blank. The key store is answered by flag so the script is about this question
    // rather than the one in front of it.
    let (code, _out, err) = invoke(&["--interactive", "--key-store", "file"], "3\n2\n\n", true);
    assert_eq!(code, ExitCode::from(2), "err: {err}");
    assert!(err.contains("is required"), "{err}");
}

/// A writer whose prompt output fails.
struct FailWriter;

impl Write for FailWriter {
    fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
        Err(io::Error::other("closed"))
    }

    fn flush(&mut self) -> io::Result<()> {
        Err(io::Error::other("closed"))
    }
}

/// A reader whose prompt read fails.
struct FailReader;

impl io::Read for FailReader {
    fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::other("no input device"))
    }
}

impl BufRead for FailReader {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        Err(io::Error::other("no input device"))
    }

    fn consume(&mut self, _amt: usize) {}
}

#[test]
fn a_prompt_whose_output_fails_is_reported() {
    // Driven to a question that reports a failed write. A menu cannot: it draws its
    // options with the errors ignored, so a closed writer shows as a menu with
    // nothing on it followed by a read that returns nothing — a different failure.
    // The SQLite route's file question is a free-text field, and `2` then a blank
    // reaches it: the choice, then the file.
    let mut reader = std::io::Cursor::new(b"2\n\n".to_vec());
    let mut out = FailWriter;
    let mut err = Vec::new();
    let code = setup_core(
        vec!["--key-store".to_owned(), "file".to_owned()],
        &mut reader,
        &mut out,
        &mut err,
        true,
        &crate::setup_preview_fixtures::no_env,
        &StubDocker::ok(),
        &crate::setup_test_seam::Probes::ok(),
    );
    assert_eq!(code, ExitCode::from(1));
    assert!(String::from_utf8_lossy(&err).contains("prompt failed"));
}

#[test]
fn a_prompt_whose_input_fails_is_reported() {
    let mut reader = FailReader;
    let mut out = Vec::new();
    let mut err = Vec::new();
    let code = setup_core(
        Vec::<String>::new(),
        &mut reader,
        &mut out,
        &mut err,
        true,
        &crate::setup_preview_fixtures::no_env,
        &StubDocker::ok(),
        &crate::setup_test_seam::Probes::ok(),
    );
    assert_eq!(code, ExitCode::from(1));
    assert!(String::from_utf8_lossy(&err).contains("prompt failed"));
}
