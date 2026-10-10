// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the reserved-surface usage lines: that every one of those
//! commands has a registered line, that the wrapping never breaks a flag name,
//! and that `ops` resolves to the subcommand that was asked about.
//!
//! ## Owns
//! - Coverage of `surface_usage_line`, `wrapped_usage`, and `ops_usage_lines`
//!
//! ## Does not own
//! - The MCP-bound usage lines (`cli_help_usage_tests`)
//! - The catalog's layout (`command_catalog_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::{SURFACE_USAGE, ops_usage_lines, surface_usage_line, wrapped_usage};

/// Every reserved-surface command resolves, so none of its lines is unreachable.
#[test]
fn every_registered_surface_command_resolves_to_its_own_line() {
    for (command, line) in SURFACE_USAGE {
        assert_eq!(
            surface_usage_line(command),
            Some(*line),
            "`{command}` must resolve to the line beside it"
        );
    }
}

/// A command that is not one of these surfaces resolves to nothing rather than to
/// another command's line.
#[test]
fn an_unregistered_command_resolves_to_nothing() {
    assert_eq!(surface_usage_line("frobnicate"), None);
}

/// The MCP-bound half is answered by the other table, so the shared lookup reads
/// both. Checked here because this file is what makes the second half exist.
#[test]
fn the_shared_lookup_reads_the_mcp_bound_half_too() {
    let full = crate::cli_help_usage::usage_for("recall").expect("recall has a usage line");
    let first = full.lines().next().expect("a usage line has a first line");
    assert_eq!(
        crate::cli_help_usage::usage_line("recall"),
        Some(first.trim_start_matches("usage: ")),
        "the shared lookup must reach the MCP-bound lines as well as these"
    );
    // It returns the flag list, not the whole entry with the prose that follows it.
    assert!(
        crate::cli_help_usage::usage_line("recall").is_some_and(|line| !line.contains('\n')),
        "the lookup returns a flag list, not the prose that follows it"
    );
}

/// The wrapping puts a continuation under the first flag, and never past the
/// column it was given.
#[test]
fn a_long_usage_line_wraps_under_its_first_flag() {
    let lines = wrapped_usage("setup", 74, 0);
    assert!(lines.len() > 1, "the setup line is long enough to wrap");
    let indent = "clio setup ".len();
    for line in &lines[1..] {
        assert!(
            line.len() > indent && line.starts_with(&" ".repeat(indent)),
            "a continuation must line up under the first flag: {line:?}"
        );
    }
    for line in &lines {
        // A single unit longer than the column is the one thing that may pass it,
        // because the alternative is splitting a flag name.
        let holds_one_unit = line.split(' ').count() <= 1;
        assert!(
            line.len() <= 74 || holds_one_unit,
            "a line ran past the column and could have wrapped: {line:?}"
        );
        // A flag and the placeholder that names its value are one unit, so a
        // continuation never opens with a bare placeholder.
        let first = line.split(' ').next().unwrap_or_default();
        assert!(
            !first.chars().next().is_some_and(char::is_uppercase),
            "a continuation opened with a bare placeholder `{first}`: {line:?}"
        );
    }
}

/// A short line is not wrapped at all, so `version` and `health` print one line.
#[test]
fn a_short_usage_line_stays_on_one_line() {
    assert_eq!(wrapped_usage("version", 74, 0), vec!["clio version"]);
    assert_eq!(wrapped_usage("health", 74, 0), vec!["clio health"]);
}

/// The lead is applied to the first line too, which is what lines the catalog's
/// `clio` words up in a column.
#[test]
fn the_lead_indents_every_line() {
    for line in wrapped_usage("mcp http", 60, 2) {
        assert!(
            line.starts_with("  "),
            "every line carries the lead: {line:?}"
        );
    }
}

/// A command with no registered line says so rather than printing an empty one.
#[test]
fn an_unregistered_command_renders_a_visible_marker() {
    let lines = wrapped_usage("frobnicate", 74, 0);
    assert_eq!(lines.len(), 1);
    assert!(
        lines[0].contains("no usage line registered"),
        "an entry with no flags reads as a command that takes none: {:?}",
        lines[0]
    );
}

/// A word longer than the column goes on its own line rather than being split,
/// because half a flag name names a flag that does not exist.
#[test]
fn a_token_too_long_for_the_column_is_not_split() {
    // A column narrower than the command name itself forces the overflow path.
    let lines = wrapped_usage("mcp http", 8, 0);
    assert!(
        lines.len() > 1,
        "a very narrow column must produce more lines"
    );
    for line in &lines {
        assert!(
            line.contains("--token-file") || !line.contains("--token-"),
            "a flag name was split: {line:?}"
        );
    }
}

/// `ops` is one verb with five subcommands, each with its own flags, and asking
/// about one returns that one rather than the union.
#[test]
fn ops_resolves_to_the_subcommand_that_was_asked_about() {
    let repair = ops_usage_lines("repair").expect("ops repair is registered");
    assert_eq!(repair.len(), 1, "{repair:?}");
    assert!(repair[0].contains("--plan"), "{repair:?}");
    assert!(repair[0].contains("--actions"), "{repair:?}");

    let diagnose = ops_usage_lines("diagnose").expect("ops diagnose is registered");
    assert_eq!(diagnose.len(), 1, "{diagnose:?}");
    assert!(
        !diagnose[0].contains("--plan"),
        "a shared line is what let `ops repair --help` hide `--plan`: {diagnose:?}"
    );
}

/// Asking about `ops` itself returns all five, in the order they are declared.
#[test]
fn ops_without_a_subcommand_returns_all_five_in_order() {
    let all = ops_usage_lines("").expect("ops has five subcommands");
    assert_eq!(all.len(), 5, "{all:?}");
    for sub in ["diagnose", "verify", "doctor", "repair", "reindex"] {
        assert!(
            all.iter()
                .any(|line| line.contains(&format!("clio ops {sub} "))),
            "`clio ops {sub}` must be among the lines: {all:?}"
        );
    }
}

/// A subcommand that does not exist resolves to nothing rather than to all five.
#[test]
fn an_unknown_ops_subcommand_resolves_to_nothing() {
    assert!(ops_usage_lines("frobnicate").is_none());
}

/// A flag and the placeholder that names its value never land on two lines.
///
/// `[--out FILE]` is two whitespace-separated tokens, and wrapping between them
/// would leave a continuation opening with `FILE]`, which reads as an argument
/// nobody can type. That is why the units are joined before the wrapping runs.
#[test]
fn a_flag_and_its_placeholder_are_one_unit() {
    let units = super::flag_units("clio backup restore ID [--yes] [--out FILE]");
    assert_eq!(
        units,
        vec!["clio", "backup", "restore", "ID", "[--yes]", "[--out FILE]",],
        "the two tokens of `[--out FILE]` must arrive as one unit, and `[--yes]` must \
         not swallow the flag that follows it"
    );
    // Re-joining the units with single spaces reproduces the input, so no text is
    // lost or reordered by the grouping.
    let rejoined = units.join(" ");
    assert_eq!(rejoined, "clio backup restore ID [--yes] [--out FILE]");
}

/// Irregular spacing must not shift a unit's byte offset and slice a flag in half.
///
/// The grouping walks the string with byte offsets, so it assumes one space between
/// tokens. A doubled space is the case that would break that assumption, and the
/// registry is hand-edited, so the assumption is checked rather than trusted.
#[test]
fn irregular_spacing_yields_whole_units_and_loses_no_text() {
    for spacing in ["  ", "   "] {
        let usage = format!("clio recall QUERY{spacing}[--out FILE] [--yes]");
        let units = super::flag_units(&usage);
        for unit in &units {
            assert!(
                unit.starts_with("clio")
                    || unit.starts_with("recall")
                    || unit.starts_with("QUERY")
                    || unit.starts_with('['),
                "a unit was cut mid-token by the spacing {spacing:?}: {unit:?}"
            );
        }
        // Nothing is lost or reordered: the units carry the input, with runs of
        // whitespace collapsed to the single space the wrapper joins them with.
        let rejoined = units.join(" ");
        assert_eq!(
            rejoined,
            usage.split_whitespace().collect::<Vec<_>>().join(" "),
            "spacing {spacing:?}"
        );
    }
}
