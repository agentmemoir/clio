// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the parity between a command's own `--help` and its line in
//! the `clio help` catalog, and the completeness of the usage registry the two
//! share.
//!
//! ## Owns
//! - Every catalog command has a registered usage line
//! - Every registered usage line's flags appear in the catalog
//! - Every catalog command resolves through `cli help <topic>`
//!
//! ## Does not own
//! - The wording of any one usage line (`cli_help_usage_tests`)
//! - The layout of the catalog text (`command_catalog`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::{ARROW, ENTRIES, NOTE_INDENT, NOTE_WIDTH, USAGE_WIDTH};
use crate::cli_help_usage;
use crate::help_flags_test_support::{entry_block, flags_in};

/// The catalog text, which is what the tests below read instead of the entry
/// table: the claim under test is about what a reader sees.
fn catalog_text() -> String {
    super::catalog_text()
}

/// The catalog's block for one command: its usage lines and its description.
fn catalog_block(command: &str) -> String {
    entry_block(&catalog_text(), command)
}

/// The reported defect: the catalog is generated from the same usage registry
/// each command's `--help` reads, so a flag cannot appear in one and not the
/// other. This is the test that keeps it that way.
///
/// The measurement that motivated it: 51 commands and 159 flags were missing
/// from their own catalog line, including `--source-type`, which a belief write
/// requires and which the catalog never mentioned.
#[test]
fn every_catalog_command_has_a_registered_usage_line() {
    let missing: Vec<&str> = ENTRIES
        .iter()
        .map(|(command, _)| *command)
        .filter(|command| cli_help_usage::usage_line(command).is_none())
        .collect();
    assert!(
        missing.is_empty(),
        "these catalog commands have no usage line, so their entry renders without \
         any flag and reads as a command that takes none: {missing:?}"
    );
}

/// Every usage line the registry holds is reachable, so a registry row is never
/// dead text.
#[test]
fn every_registered_usage_line_is_reachable_from_a_command() {
    // `usage_for` is the MCP-bound half of the registry; the rest is the surface
    // table. Both are covered by the loop below over the catalog, which is the
    // only consumer of the surface half.
    let unreachable: Vec<&str> = ENTRIES
        .iter()
        .map(|(command, _)| *command)
        .filter(|command| {
            cli_help_usage::usage_line(command).is_none()
                && crate::cli_help::verb_help(command).is_some()
        })
        .collect();
    assert!(
        unreachable.is_empty(),
        "these commands have a verb help string but no registry entry: {unreachable:?}"
    );
}

/// Every command the dispatcher will run has a catalog entry.
///
/// This is the direction the other parity tests cannot see. They compare the
/// catalog against the usage registry, so a command missing from *both* passes
/// them. It happened: the table conversion split the old `compose up|down` line on
/// a spaced `|`, which `up|down` does not have, and `clio compose down` vanished
/// from the catalog while every flag check stayed green.
#[test]
fn every_command_the_dispatcher_runs_is_in_the_catalog() {
    let listed: Vec<&str> = ENTRIES.iter().map(|(command, _)| *command).collect();
    let missing: Vec<&str> = crate::cli_help::all_known_commands()
        .into_iter()
        .filter(|command| !listed.contains(command))
        .collect();
    assert!(
        missing.is_empty(),
        "these commands the CLI will run are not in `clio help`, so a reader cannot \
         find them: {missing:?}"
    );
}

/// A catalog entry names a command the CLI will run, so the catalog cannot drift
/// into offering something that does not exist.
#[test]
fn every_catalog_command_is_one_the_dispatcher_runs() {
    let known = crate::cli_help::all_known_commands();
    let unknown: Vec<&str> = ENTRIES
        .iter()
        .map(|(command, _)| *command)
        .filter(|command| !known.contains(command))
        .collect();
    assert!(
        unknown.is_empty(),
        "`clio help` lists commands the CLI will refuse: {unknown:?}"
    );
}

/// A flag in a command's usage line appears in that command's catalog block.
///
/// This reads the rendered catalog rather than the entry table, so it also fails
/// if the renderer ever stops printing the registry's flags.
#[test]
fn every_flag_in_a_usage_line_appears_in_that_commands_catalog_line() {
    for (command, _) in ENTRIES {
        let Some(usage) = cli_help_usage::usage_line(command) else {
            continue;
        };
        let block = catalog_block(command);
        let missing: Vec<String> = flags_in(usage)
            .into_iter()
            .filter(|flag| !block.contains(flag.as_str()))
            .collect();
        assert!(
            missing.is_empty(),
            "`clio help` does not show {missing:?} for `clio {command}`, though its \
             own `--help` does:\n{block}"
        );
    }
}

/// The reverse direction: no description names a flag that no command accepts.
///
/// A description may name a flag its own command refuses — `backup list` says so
/// about `--db` — so the check is against the whole catalog rather than against one
/// command. What it catches is a flag name that exists nowhere, which is a
/// description promising something the reader cannot type.
#[test]
fn a_description_names_no_flag_the_cli_does_not_accept() {
    let accepted: Vec<String> = ENTRIES
        .iter()
        .filter_map(|(command, _)| cli_help_usage::usage_line(command))
        .flat_map(flags_in)
        .collect();
    let unknown: Vec<(String, String)> = ENTRIES
        .iter()
        .flat_map(|(command, note)| {
            flags_in(note)
                .into_iter()
                .filter(|flag| !accepted.contains(flag))
                .map(|flag| (command.to_string(), flag))
                .collect::<Vec<(String, String)>>()
        })
        .collect();
    assert!(
        unknown.is_empty(),
        "these descriptions name a flag no command accepts, so a reader is told to \
         type something that does not exist: {unknown:?}"
    );
}

/// Every command the catalog lists resolves through `clio help <topic>`, so the
/// topic lookup and the catalog cannot drift apart either.
#[test]
fn every_catalog_command_is_a_known_command() {
    let unknown: Vec<&str> = ENTRIES
        .iter()
        .map(|(command, _)| *command)
        .filter(|command| !crate::cli_help::is_known_command(command))
        .collect();
    assert!(
        unknown.is_empty(),
        "these catalog commands are not recognised as commands, so `clio help` \
         advertises a command that cannot be reached: {unknown:?}"
    );
}

/// The catalog's install and uninstall notes cannot name a subset of the
/// supported harnesses as if it were the whole list.
///
/// The measurement that motivated it: `clio help` described both verbs with
/// "(currently claude)" while the harness registry held seven harnesses and each
/// command's own `--help` listed them all.
#[test]
fn install_and_uninstall_catalog_notes_name_no_harness_subset() {
    let supported = crate::agent_harness::supported_names();
    let offenders: Vec<(String, Vec<String>)> = ENTRIES
        .iter()
        .filter(|(command, _)| *command == "install" || *command == "uninstall")
        .map(|(command, note)| {
            let named: Vec<String> = supported
                .iter()
                .copied()
                .filter(|name| note.contains(name))
                .map(ToString::to_string)
                .collect();
            (command.to_string(), named)
        })
        .filter(|(_, named)| !named.is_empty() && named.len() != supported.len())
        .collect();
    assert!(
        offenders.is_empty(),
        "`clio help`'s install/uninstall notes name the subset {offenders:?} of the \
         {} supported harnesses, so a reader is told a partial list is the whole one",
        supported.len()
    );
}

/// The wrapping never cuts a flag name in half, because half a flag name names a
/// flag that does not exist.
#[test]
fn wrapping_never_splits_a_flag_name() {
    let all: Vec<String> = ENTRIES
        .iter()
        .flat_map(|(command, note)| {
            flags_in(cli_help_usage::usage_line(command).unwrap_or_default())
                .into_iter()
                .chain(flags_in(note))
                .collect::<Vec<String>>()
        })
        .collect();
    let text = catalog_text();
    for flag in all {
        assert!(
            text.contains(&flag),
            "`{flag}` is listed in a usage line or a description but the rendered \
             catalog does not contain it whole, so a flag name was split by wrapping"
        );
    }
}

/// The catalog's two wrap columns are pinned as literals: 84 for the usage lines
/// and 92 for the note lines, indent included. Retuning either constant changes
/// what a reader sees, so it must change this test in the same commit.
#[test]
fn the_catalog_widths_are_pinned_to_their_columns() {
    assert_eq!(USAGE_WIDTH, 84, "usage lines wrap at 84 columns");
    assert_eq!(NOTE_WIDTH, 92, "note lines wrap at 92 columns");
}

/// No rendered catalog line runs past the column it was wrapped to.
///
/// The bounds are the rendered columns themselves, so this also fails if the
/// renderer stops applying a width. The renderer appends the tool arrow to a
/// command's last usage line after wrapping, so that suffix is stripped before
/// the check: the wrapped text must fit 84, and the arrow is the only part
/// allowed to pass it. Note lines carry no suffix and must stay within 92.
#[test]
fn no_rendered_catalog_line_exceeds_its_width() {
    let arrows: Vec<String> = crate::cli_help::command_bindings()
        .iter()
        .map(|(_, tool)| format!("{ARROW}{tool}"))
        .collect();
    let text = catalog_text();
    let over: Vec<(usize, &str)> = text
        .lines()
        .filter_map(|line| {
            let leading = line.len() - line.trim_start_matches(' ').len();
            if leading == NOTE_INDENT.len() {
                return (line.len() > 92).then_some((line.len(), line));
            }
            let wrapped = arrows
                .iter()
                .find_map(|arrow| line.strip_suffix(arrow.as_str()))
                .unwrap_or(line);
            (wrapped.len() > 84).then_some((wrapped.len(), line))
        })
        .collect();
    assert!(
        over.is_empty(),
        "these catalog lines' wrapped text runs past the 84-column usage or \
         92-column note bound: {over:?}"
    );
}
