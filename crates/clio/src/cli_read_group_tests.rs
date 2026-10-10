// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the read-command engine: two-token group resolution,
//! group help, unknown-subcommand suggestion, and top-level dispatch.
//!
//! ## Owns
//! - Assertions over `run_verb`/`run_with_state` command resolution
//!
//! ## Does not own
//! - Any command group's implementation (see the `cli_read_*` modules)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::{CliIo, run_verb};
use crate::cli_read_test_support::{run_open, run_state, state, tokens};
use std::process::ExitCode;

fn run_cli(cmd: &str, args: &[&str], is_tty: bool) -> (ExitCode, String, String) {
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
        run_verb(cmd, tokens(args), &mut io)
    };
    (
        code,
        String::from_utf8(out).expect("utf8"),
        String::from_utf8(err).expect("utf8"),
    )
}

#[test]
fn group_subcommand_help_resolves() {
    let (code, out, err) = run_cli("task", &["get", "--help"], true);
    assert_eq!(code, ExitCode::from(0));
    assert!(out.contains("usage: clio task get"), "{out}");
    assert!(err.is_empty(), "{err}");

    let (code, out, _) = run_cli("history", &["temporal", "--help"], false);
    assert_eq!(code, ExitCode::from(0));
    assert!(out.contains("usage: clio history temporal"), "{out}");
}

#[test]
fn group_help_lists_subcommands_without_opening_a_database() {
    let (code, out, err) = run_cli("task", &["--help"], true);
    assert_eq!(code, ExitCode::from(0));
    assert!(out.contains("`task` subcommands:"), "{out}");
    assert!(out.contains("clio task get"), "{out}");
    assert!(out.contains("task_get"), "{out}");
    assert!(err.is_empty(), "{err}");
}

#[test]
fn group_requires_a_subcommand() {
    let (code, out, err) = run_cli("task", &[], false);
    assert_eq!(code, ExitCode::from(2));
    assert!(out.is_empty(), "{out:?}");
    assert!(err.contains("requires a subcommand"), "{err}");
}

#[test]
fn unknown_subcommand_suggests_a_neighbour() {
    let (code, out, err) = run_cli("task", &["gett"], false);
    assert_eq!(code, ExitCode::from(2));
    assert!(out.is_empty(), "{out:?}");
    assert!(err.contains("did you mean `clio task get`?"), "{err}");

    let (code, _, err) = run_cli("task", &["zzzzzz"], false);
    assert_eq!(code, ExitCode::from(2));
    assert!(err.contains("try `clio task get`"), "{err}");
}

#[test]
fn run_open_dispatches_group_and_single_commands() {
    // A group command against an empty in-memory store succeeds (record null).
    let (code, out, err) = run_open(
        "task",
        &["get", "t1", "--db", "sqlite::memory:", "--bank", "bank-a"],
        false,
    );
    assert_eq!(code, ExitCode::from(0));
    assert!(out.contains("\"ok\":true"), "{out}");
    assert!(err.is_empty(), "{err}");

    // A single read command still opens state and runs.
    let (code, out, _) = run_open("stats", &["--db", "sqlite::memory:"], false);
    assert_eq!(code, ExitCode::from(0));
    assert!(out.contains("\"ok\":true"), "{out}");
}

#[test]
fn run_with_state_covers_every_group() {
    let s = state();
    // Each canonical verb resolves to its group and reaches the tool; unknown
    // ids/empty state produce tool errors rather than usage errors.
    let verbs = [
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
        "audit trail",
    ];
    for verb in verbs {
        let args: &[&str] = match verb {
            "history temporal" => &["triple:s:p", "--bank", "bank-a"],
            "intent gate" => &["hello", "--bank", "bank-a"],
            "memtree query" | "persona get" | "maintenance status" => &["--bank", "bank-a"],
            _ => &["ghost", "--bank", "bank-a"],
        };
        let (code, out, err) = run_state(verb, args, &s, false);
        assert!(
            code == ExitCode::from(0) || code == ExitCode::from(1),
            "{verb}: code={code:?} out={out} err={err}"
        );
    }
}
