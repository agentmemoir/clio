// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the help surfaces' width and binding contract against the
//! real `clio` binary: help output does not depend on `COLUMNS`, a bound
//! command's line names its tool, an unbound one names none, group help names
//! each subcommand with its binding, and a leaf's `--help` shows its options
//! block with defaults and globals.
//!
//! ## Owns
//!
//! - End-to-end assertions over spawned `clio` processes for help wrapping,
//!   bound/unbound markers, and group help
//!
//! ## Does not own
//!
//! - The string-level wrapping tests (`cli_help_surfaces_tests`,
//!   `command_catalog_tests`)
//! - The golden help capture (`golden_capture_test`)
//!
//! ## Boundary
//!
//! Tests here break when the help surfaces' width or binding contract changes;
//! they never change it.
//!
//! Keep this module focused on the process contract of the help surfaces.

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

/// Run one command with `COLUMNS` set on the child only, or unset when `None`.
fn run_with_columns(args: &[&str], columns: Option<&str>) -> Output {
    let mut cmd = clio();
    if let Some(columns) = columns {
        cmd.env("COLUMNS", columns);
    }
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn clio")
}

fn stdout_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Help wrapping is fixed-width: the same surface prints byte-identical output
/// whatever `COLUMNS` the caller's terminal reports.
///
/// The renderer reads no terminal state, so a narrow emulator and a wide one
/// see the same bytes. The surfaces span the catalog, a bound verb's own help
/// with its options block, a long-note topic, and two group helps.
#[test]
fn help_output_does_not_depend_on_the_columns_environment() {
    let surfaces: &[&[&str]] = &[
        &["help"],
        &["recall", "--help"],
        &["help", "sync", "keygen"],
        &["backup", "--help"],
        &["ops", "--help"],
    ];
    for surface in surfaces {
        let narrow = stdout_of(&run_with_columns(surface, Some("40")));
        let wide = stdout_of(&run_with_columns(surface, Some("300")));
        let unset = stdout_of(&run_with_columns(surface, None));
        assert!(
            !narrow.is_empty(),
            "`clio {}` printed nothing, so the equality below proves nothing",
            surface.join(" ")
        );
        assert_eq!(
            narrow,
            wide,
            "`COLUMNS` changed the bytes of `clio {}`",
            surface.join(" ")
        );
        assert_eq!(
            narrow,
            unset,
            "an unset `COLUMNS` changed the bytes of `clio {}`",
            surface.join(" ")
        );
    }
}

/// A bound command's catalog line names its MCP tool, on the same line its
/// wrapped flags end on.
#[test]
fn a_bound_commands_catalog_line_names_its_tool() {
    let out = stdout_of(&run(&["help"]));
    let lines: Vec<&str> = out.lines().collect();
    let start = lines
        .iter()
        .position(|line| line.trim_start().starts_with("clio recall "))
        .expect("the catalog lists `clio recall`");
    let block: Vec<&str> = std::iter::once(lines[start])
        .chain(
            lines[start + 1..]
                .iter()
                .copied()
                .take_while(|line| !line.trim_start().starts_with("clio ")),
        )
        .collect();
    assert!(
        block.iter().any(|line| line.contains("clio recall QUERY")),
        "the block read is recall's: {block:?}"
    );
    let arrow_line = block
        .iter()
        .find(|line| line.contains("-> retrieve"))
        .unwrap_or_else(|| panic!("the bound tool must be named in the block: {block:?}"));
    assert!(
        arrow_line.trim_end().ends_with("-> retrieve"),
        "the bound tool must close its line: {arrow_line}"
    );
}

/// An unbound command's catalog block names no tool: the flags end the entry
/// and no arrow follows.
#[test]
fn an_unbound_commands_catalog_block_names_no_tool() {
    let out = stdout_of(&run(&["help"]));
    let lines: Vec<&str> = out.lines().collect();
    let start = lines
        .iter()
        .position(|line| line.trim_start().starts_with("clio backup create"))
        .expect("the catalog lists `clio backup create`");
    let block: Vec<&str> = std::iter::once(lines[start])
        .chain(
            lines[start + 1..]
                .iter()
                .copied()
                .take_while(|line| !line.trim_start().starts_with("clio ")),
        )
        .collect();
    assert!(
        block.join("\n").contains("[--db PATH]"),
        "the block read is backup create's: {block:?}"
    );
    assert!(
        !block.iter().any(|line| line.contains("->")),
        "an unbound command's block names no tool: {block:?}"
    );
}

/// A group's `--help` names each subcommand with the tool it binds.
///
/// `task` is the group where every subcommand binds a tool; the marker is the
/// arrow, so a dropped binding prints nothing where the tool should be.
#[test]
fn a_bound_group_help_names_each_subcommands_tool() {
    let out = stdout_of(&run(&["task", "--help"]));
    for (sub, tool) in [("get", "task_get"), ("history", "task_history")] {
        let line = out
            .lines()
            .find(|line| line.trim_start().starts_with(&format!("clio task {sub}")))
            .unwrap_or_else(|| panic!("group help lists `clio task {sub}`: {out}"));
        assert!(
            line.contains(&format!("-> {tool}")),
            "`clio task {sub}` must name its tool in group help: {line}"
        );
    }
}

/// The same group help says `(no MCP tool)` beside a subcommand that binds
/// none, rather than printing a meaningless arrow.
#[test]
fn an_unbound_group_help_says_so_beside_its_subcommand() {
    let out = stdout_of(&run(&["backup", "--help"]));
    let line = out
        .lines()
        .find(|line| line.trim_start().starts_with("clio backup create"))
        .expect("group help lists `clio backup create`");
    assert!(
        line.contains("(no MCP tool)"),
        "an unbound subcommand's group line must say so: {line}"
    );
}

/// A leaf's `--help` shows the options block: each documented flag on one line
/// with its description, and a value flag's default in brackets.
#[test]
fn a_leaf_help_line_shows_its_description_and_default() {
    let out = stdout_of(&run(&["recall", "--help"]));
    let line = out
        .lines()
        .find(|line| line.starts_with("  --limit "))
        .unwrap_or_else(|| panic!("the options block lists `--limit`: {out}"));
    assert!(
        line.ends_with("[default: 10]"),
        "the value flag's default must close its help line: {line}"
    );
}

/// The same options block separates the globals a record accepts but never
/// documents, under their own heading.
#[test]
fn a_leaf_help_shows_its_globals_under_their_own_heading() {
    let out = stdout_of(&run(&["recall", "--help"]));
    let globals = out
        .lines()
        .position(|line| line == "Global flags:")
        .unwrap_or_else(|| panic!("the options block separates the globals: {out}"));
    assert!(
        out.lines()
            .skip(globals + 1)
            .any(|line| line.starts_with("  --db ")),
        "a declared global renders under `Global flags:`"
    );
}

/// A help topic the CLI does not know fails explicitly instead of printing the
/// whole catalog, which is what made the old failure unreadable.
#[test]
fn an_unknown_help_topic_fails_instead_of_printing_the_catalog() {
    let output = run(&["help", "frobnicate"]);
    assert_eq!(
        output.status.code(),
        Some(2),
        "an unknown topic is a usage error"
    );
    let err = stderr_of(&output);
    assert!(
        err.contains("unknown command `frobnicate`"),
        "the failure names the unknown topic: {err}"
    );
    assert!(
        !stdout_of(&output).contains("clio health"),
        "no catalog leaks on a failed topic lookup"
    );
}
