// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the shared runtime-backed command engine: subcommand
//! resolution, `--help`, unknown flags/subcommands, and envelope error mapping.
//!
//! ## Owns
//! - Engine-level behavior tests and the shared runtime-group test runner
//!
//! ## Does not own
//! - The engine implementation under test (`runtime_cli`)
//! - Config/ranking semantics (see `clio-config`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;
use crate::cli_read::CliIo;
use crate::config_cli::ConfigGroup;
use serde_json::json;

/// Tokenize string args for the engine.
pub(crate) fn tokens(args: &[&str]) -> Vec<String> {
    args.iter().map(|arg| (*arg).to_owned()).collect()
}

/// Run one runtime-backed group against an injected `Runtime`.
pub(crate) fn run_group(
    group: &dyn RuntimeGroup,
    cmd: &str,
    args: &[&str],
    runtime: &mut Runtime,
    is_tty: bool,
) -> (ExitCode, String, String) {
    let env = |_key: &str| -> Option<String> { None };
    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut input = std::io::empty();
    let code = {
        let mut io = CliIo {
            out: &mut out,
            err: &mut err,
            stdin: &mut input,
            is_tty,
            no_color: true,
            env: &env,
        };
        run(group, cmd, tokens(args), &mut io, runtime)
    };
    (
        code,
        String::from_utf8(out).expect("utf8"),
        String::from_utf8(err).expect("utf8"),
    )
}

/// A hermetic runtime with no host env and no deployment overlay.
pub(crate) fn runtime() -> Runtime {
    Runtime::with_env(&|_key: &str| None)
}

#[test]
fn missing_subcommand_exits_two_and_names_the_first() {
    let (code, out, err) = run_group(&ConfigGroup, "config", &[], &mut runtime(), false);
    assert_eq!(code, ExitCode::from(2), "{out}{err}");
    assert!(err.contains("requires a subcommand"), "{err}");
    assert!(err.contains("clio config get"), "{err}");
}

#[test]
fn unknown_subcommand_suggests_the_closest_verb() {
    let (code, out, err) = run_group(&ConfigGroup, "config", &["gett"], &mut runtime(), false);
    assert_eq!(code, ExitCode::from(2), "{out}{err}");
    assert!(err.contains("unknown `config` subcommand `gett`"), "{err}");
    assert!(err.contains("did you mean `clio config get`?"), "{err}");
}

#[test]
fn group_help_and_verb_help_exit_zero_without_calling_a_tool() {
    let (code, out, err) = run_group(&ConfigGroup, "config", &["--help"], &mut runtime(), true);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert!(out.contains("`config` subcommands:"), "{out}");
    assert!(out.contains("clio config get"), "{out}");

    let (code, out, err) = run_group(
        &ConfigGroup,
        "config",
        &["get", "--help"],
        &mut runtime(),
        true,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert!(out.contains("usage: clio config get"), "{out}");
    assert!(out.contains("calls the `config_get` tool"), "{out}");
}

#[test]
fn unknown_flag_fails_closed_with_exit_two() {
    let (code, out, err) = run_group(
        &ConfigGroup,
        "config",
        &["get", "--bogus"],
        &mut runtime(),
        false,
    );
    assert_eq!(code, ExitCode::from(2), "{out}{err}");
    assert!(err.contains("unknown flag `--bogus`"), "{err}");
}

#[test]
fn explicit_output_flag_wins_on_a_parse_error() {
    let (code, out, err) = run_group(
        &ConfigGroup,
        "config",
        &["get", "--output", "json", "--bogus"],
        &mut runtime(),
        true,
    );
    assert_eq!(code, ExitCode::from(2), "{out}{err}");
    assert!(out.is_empty(), "{out}");
    assert!(err.contains("\"code\":\"usage\""), "{err}");
}

#[test]
fn output_error_and_no_suggestion_paths() {
    let (code, _, err) = run_group(
        &ConfigGroup,
        "config",
        &["get", "--output", "xml"],
        &mut runtime(),
        false,
    );
    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("--output must be"), "{err}");

    let (code, _, err) = run_group(&ConfigGroup, "config", &["zzzz"], &mut runtime(), false);
    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("try `clio config get`"), "{err}");
}

#[test]
fn envelope_error_maps_to_operational_exit_one() {
    let error = envelope_error(&json!({"ok": false, "code": "unknown_path", "message": "nope"}));
    assert_eq!(error.exit_code(), ExitCode::from(1));
    let error = envelope_error(&json!({"ok": false}));
    assert_eq!(error.exit_code(), ExitCode::from(1));
}

#[test]
fn no_positionals_and_pretty_helpers_behave() {
    let parsed = cli_args::parse(&FlagSpec::new(&[], &[]), tokens(&["x"])).expect("parse");
    assert!(no_positionals(&parsed).is_err());
    let empty = cli_args::parse(&FlagSpec::new(&[], &[]), tokens(&[])).expect("parse");
    assert!(no_positionals(&empty).is_ok());
    assert_eq!(
        pretty_without_ok(&json!({"ok": true, "a": 1})),
        "{\n  \"a\": 1\n}"
    );
}
