// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the read-command help and error-output edge paths through
//! `run_verb`: subcommand `--help`, explicit `--output` on a parse failure, and
//! the missing-positional message.
//!
//! ## Owns
//! - Assertions over `run_verb` help and parse-failure rendering
//!
//! ## Does not own
//! - The read-command implementation under test (`cli_read`)
//! - The in-process read behavior tests (`cli_read_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;

fn tokens(args: &[&str]) -> Vec<String> {
    args.iter().map(|arg| (*arg).to_owned()).collect()
}

/// Drive `run_verb` over injected streams with a chosen terminal state.
fn run_cli(verb: &str, args: &[&str], is_tty: bool) -> (ExitCode, String, String) {
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
        run_verb(verb, tokens(args), &mut io)
    };
    (
        code,
        String::from_utf8(out).expect("utf8"),
        String::from_utf8(err).expect("utf8"),
    )
}

#[test]
fn subcommand_help_renders_usage_and_exits_zero() {
    for verb in [
        "recall",
        "get",
        "show",
        "inspect",
        "stats",
        "task get",
        "task history",
        "failure list",
        "memtree query",
        "memtree get",
        "history temporal",
        "triple query",
        "belief history",
        "graph query",
        "associations",
        "persona get",
        "maintenance status",
        "intent gate",
        "compose-context",
        "audit trail",
    ] {
        let (code, out, err) = run_cli(verb, &["--help"], true);
        assert_eq!(code, ExitCode::from(0), "{verb}");
        assert!(out.contains("usage: clio "), "{verb}: {out}");
        assert!(out.contains("calls the `"), "{verb}: {out}");
        assert!(err.is_empty(), "{verb} stderr: {err}");

        let (short_code, short_out, _) = run_cli(verb, &["-h"], false);
        assert_eq!(short_code, ExitCode::from(0), "{verb} -h");
        assert!(short_out.contains("usage: clio "), "{verb} -h: {short_out}");
    }
    // An unknown verb's `--help` still fails as an unknown command. The
    // explicit in-memory DB keeps the test from creating a default DB file.
    let (code, out, err) = run_cli("bogus", &["--help", "--db", "sqlite::memory:"], false);
    assert_eq!(code, ExitCode::from(2));
    assert!(out.is_empty(), "{out}");
    assert!(err.contains("\"code\":\"usage\""), "{err}");
}

#[test]
fn group_help_and_unknown_subcommand() {
    let (code, out, err) = run_cli("task", &["--help"], true);
    assert_eq!(code, ExitCode::from(0));
    assert!(out.contains("`task` subcommands:"), "{out}");
    assert!(err.is_empty(), "{err}");

    let (code, out, err) = run_cli("task", &["bogus", "--db", "sqlite::memory:"], false);
    assert_eq!(code, ExitCode::from(2));
    assert!(out.is_empty(), "{out}");
    assert!(err.contains("unknown `task` subcommand"), "{err}");
}

#[test]
fn subcommand_help_does_not_open_the_database() {
    // No `--db` is given; help must return before `McpState::open` is reached.
    let (code, out, err) = run_cli("stats", &["--help"], false);
    assert_eq!(code, ExitCode::from(0));
    assert!(out.contains("usage: clio stats"), "{out}");
    assert!(err.is_empty(), "{err}");
}

#[test]
fn parse_error_honors_an_explicit_output_flag() {
    // Explicit `--output text` while piped must still print human text.
    let (code, out, err) = run_cli("recall", &["q", "--output", "text", "--limt", "2"], false);
    assert_eq!(code, ExitCode::from(2));
    assert!(out.is_empty(), "{out:?}");
    assert!(err.starts_with("error:"), "{err}");

    // Explicit `--output json` on a TTY must still print the envelope.
    let (code, out, err) = run_cli("recall", &["q", "--output", "json", "--limt", "2"], true);
    assert_eq!(code, ExitCode::from(2));
    assert!(out.is_empty(), "{out:?}");
    assert!(err.contains("\"code\":\"usage\""), "{err}");

    // The `-o` alias wins on a parse failure exactly like the long flag.
    let (code, out, err) = run_cli("recall", &["q", "-o", "text", "--limt", "2"], false);
    assert_eq!(code, ExitCode::from(2));
    assert!(out.is_empty(), "{out:?}");
    assert!(err.starts_with("error:"), "{err}");

    let (code, out, err) = run_cli("recall", &["q", "-o=json", "--limt", "2"], true);
    assert_eq!(code, ExitCode::from(2));
    assert!(out.is_empty(), "{out:?}");
    assert!(err.contains("\"code\":\"usage\""), "{err}");
}

#[test]
fn missing_positional_uses_the_correct_article() {
    let (code, _out, err) = run_cli("get", &["--db", "sqlite::memory:"], false);
    assert_eq!(code, ExitCode::from(2));
    assert!(err.contains("requires an ID argument"), "{err}");

    let (_, _, err) = run_cli("recall", &["--db", "sqlite::memory:"], false);
    assert!(err.contains("requires a QUERY argument"), "{err}");
}
