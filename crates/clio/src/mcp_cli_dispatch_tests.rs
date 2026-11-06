// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `clio mcp` entry point's own dispatch branches: the help
//! spellings, and the fail-closed flag check on a subcommand.
//!
//! ## Owns
//! - Dispatch, exit-code, and record-help-bytes tests for the `mcp` entry
//!   point's own branches
//!
//! ## Does not own
//! - The subcommand handlers and flag decoding (`mcp_cli_tests`)
//! - The credential-store load (`mcp_cli_credentials_tests`)
//!
//! ## Boundary
//!
//! Tests here break when a `clio mcp` dispatch branch or its exit code changes;
//! they never change it.
//!
//! Keep this module focused on the `clio mcp` dispatch surface.

use super::tests::Scratch;
use super::tests::args_of;
use super::*;
use std::fs;
use std::path::Path;

/// `--tools` narrows the pack the handler builds: the minimal preset's tool set
/// and no tool outside it, through the same write path the export uses.
#[test]
fn schema_export_narrows_the_pack_to_the_tools_flag() {
    let scratch = Scratch::new("tools-minimal");
    assert_eq!(
        mcp_schema_export(args_of(&[
            "--tools",
            "minimal",
            "--out",
            &scratch.display()
        ])),
        ExitCode::SUCCESS
    );
    let raw = fs::read_to_string(scratch.path()).expect("written pack");
    let pack: serde_json::Value = serde_json::from_str(&raw).expect("json");
    let mut published: Vec<&str> = pack["tools"]
        .as_array()
        .expect("a tools array")
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    published.sort_unstable();
    let mut expected = clio_types::ToolPreset::Minimal.tools();
    expected.sort_unstable();
    assert_eq!(published, expected);
    assert!(
        !published.contains(&"store"),
        "a tool outside the minimal preset shipped in the pack: {published:?}"
    );
    assert!(
        published.len()
            < clio_mcp::schema::schema_pack()["tools"]
                .as_array()
                .expect("a tools array")
                .len(),
        "the flag did not narrow the pack"
    );
}

/// A `--tools` value no preset and no group list can honour stops the export
/// with the usage refusal the transports give it, so no pack is printed for a
/// selection this build cannot build.
#[test]
fn schema_export_refuses_an_unknown_tools_value() {
    for value in ["mnimal", "custom:bogus"] {
        assert_eq!(
            mcp(args_of(&["schema-export", "--tools", value])),
            ExitCode::from(2),
            "`{value}` was accepted"
        );
    }
    let error = crate::mcp_tools_flag::selection(&crate::cli_flags_test_support::flags_of(&[
        "--tools", "mnimal",
    ]))
    .expect_err("the value names no preset this build knows");
    assert!(
        error
            .message()
            .contains("--tools `mnimal` is not a preset this build knows"),
        "{}",
        error.message()
    );
}

/// Every help spelling the `mcp` entry point accepts prints the subcommand list
/// and succeeds, which is what `clio mcp --help` used to fail to do.
#[test]
fn a_help_spelling_lists_the_subcommands() {
    for spelling in ["help", "--help", "-h"] {
        assert_eq!(mcp(args_of(&[spelling])), ExitCode::SUCCESS, "{spelling}");
    }
}

/// An undeclared flag on a subcommand fails closed, rather than printing the
/// whole schema pack as if the flag had been understood.
#[test]
fn schema_export_rejects_an_undeclared_flag() {
    assert_eq!(
        mcp(args_of(&["schema-export", "--bogus"])),
        ExitCode::from(2)
    );
}

/// Asking `schema-export` for help renders from its record. It used to fall
/// through and print the whole schema pack, so the answer was 70 KB of JSON
/// that never said what the command takes.
#[test]
fn schema_export_answers_help_from_its_record() {
    // Both help spellings take the record branch, not the pack fall-through.
    for spelling in ["--help", "-h"] {
        assert_eq!(
            mcp(args_of(&["schema-export", spelling])),
            ExitCode::SUCCESS,
            "{spelling}"
        );
    }
    // The bytes the handler prints are the record's usage line and notes, not
    // the schema pack: the old fall-through printed a pack holding
    // `am_tool_schema_pack` and none of the markers below.
    let text = schema_export_help_text();
    let usage = crate::cli_help_usage::usage_line("mcp schema-export")
        .expect("the record registers a usage line");
    assert!(text.contains(&usage), "usage line missing: {text}");
    assert!(
        text.contains("--out FILE writes the pack to a file"),
        "record notes missing: {text}"
    );
    assert!(
        !text.contains("am_tool_schema_pack"),
        "help must not print the pack: {text}"
    );
}

/// Read a written pack back and pin the export shape: it parses to exactly the
/// pack the handler built, and each `tools` array element sits on one line.
fn assert_written_pack(path: &Path) {
    let raw = fs::read_to_string(path).expect("written pack");
    let pack: serde_json::Value = serde_json::from_str(&raw).expect("json");
    assert_eq!(pack, clio_mcp::schema::schema_pack());
    assert_eq!(pack["format"], "am_tool_schema_pack");
    let tools = pack["tools"].as_array().expect("a tools array");
    assert!(!tools.is_empty(), "the catalog has tools");
    let lines: Vec<&str> = raw.lines().collect();
    let start = lines
        .iter()
        .position(|line| line.trim_start().starts_with("\"tools\""))
        .expect("a tools key");
    assert!(
        lines[start].trim_end().ends_with('['),
        "tools opens an array"
    );
    let mut seen = 0;
    for line in &lines[start + 1..] {
        let trimmed = line.trim();
        if trimmed == "]" || trimmed == "]," {
            break;
        }
        let element: serde_json::Value =
            serde_json::from_str(trimmed.trim_end_matches(',')).expect("one element per line");
        assert!(element.is_object(), "a tool element is an object");
        seen += 1;
    }
    assert_eq!(seen, tools.len(), "one line per tools element");
}

/// `--out` runs the real write path: the file exists, parses back to the pack
/// the handler built, and keeps one tools element per line.
#[test]
fn schema_export_writes_a_pretty_pack_to_the_out_flag() {
    let scratch = Scratch::new("out");
    assert_eq!(
        mcp_schema_export(args_of(&["--out", &scratch.display()])),
        ExitCode::SUCCESS
    );
    assert!(scratch.path().is_file(), "--out wrote no file");
    assert_written_pack(scratch.path());
}

/// `--path` is an alias for `--out` in the same write path, so it must produce
/// the same file shape.
#[test]
fn schema_export_accepts_path_as_an_alias_for_out() {
    let scratch = Scratch::new("path");
    assert_eq!(
        mcp_schema_export(args_of(&["--path", &scratch.display()])),
        ExitCode::SUCCESS
    );
    assert!(scratch.path().is_file(), "--path wrote no file");
    assert_written_pack(scratch.path());
}

/// An undeclared flag on the `stdio` transport is a usage error (exit 2), checked
/// before the store is opened; it used to surface as an exit-1 open failure.
#[test]
fn stdio_rejects_an_undeclared_flag() {
    assert_eq!(mcp(args_of(&["stdio", "--bogus"])), ExitCode::from(2));
}

/// A flag only the HTTP transport takes is refused on `stdio` rather than
/// silently ignored, so a wrong flag cannot look like a working invocation.
#[test]
fn stdio_rejects_an_http_only_flag() {
    assert_eq!(
        mcp(args_of(&["stdio", "--bind", "127.0.0.1:1"])),
        ExitCode::from(2)
    );
}

/// The removed `--auth-token` flag is now an unknown flag on `http`: a usage
/// error checked before the store is opened, not a silent credential.
#[test]
fn http_rejects_the_removed_auth_token_flag() {
    assert_eq!(
        mcp(args_of(&["http", "--auth-token", "x"])),
        ExitCode::from(2)
    );
}
