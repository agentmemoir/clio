// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing that the CLI usage-line table is
//! keyed one-entry-per-verb, so every string in it is reachable, and for
//! pinning usage-line content against the parser and against removed routes.
//!
//! ## Owns
//! - The no-duplicate-verb invariant, the "a looked-up verb resolves to its
//!   own text" check, and the "only an unregistered verb has a row here" pin
//! - The both-direction flag agreement pin: every flag a command's help text
//!   names is one its parser accepts, and every non-global flag its parser
//!   accepts is named by the command's record, pinned per command so a
//!   command that loses its parser or its help text fails, with the record
//!   contributor count and the authored-row contributor count pinned apart
//! - The never-advertise-a-removed-route pin: the transfer usage lines name the
//!   store and `CLIO_SYNC_KEY_FILE` routes and never advertise `--sync-key` or
//!   the removed `CLIO_SYNC_KEY` environment rung
//! - The consolidate rename pin: the two help strings the rename touched keep
//!   advertising `--bank` and never advertise the removed `scope` again
//!
//! ## Does not own
//! - The wording of any other usage line
//! - The help catalog and tool bindings (`cli_help_tests`)
//! - The literal-read scan over the flag-reading modules (`flag_read_scan_test`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use std::collections::BTreeSet;

use super::{VERB_USAGE, usage_for};
use crate::cli_help;
use crate::cli_help_usage;
use crate::help_flags_test_support::flags_in;

/// `VERB_USAGE` once held two entries keyed `backup restore`, and `usage_for`
/// returns the first match, so the second was text no command, flag, or output
/// mode could print. A maintainer who rewrote it saw no change and no
/// diagnostic. `usage_for` has no way to tell two variants of one verb apart,
/// so the table must hold one entry per verb.
#[test]
fn every_verb_appears_exactly_once_in_the_usage_table() {
    let mut seen = BTreeSet::new();
    let mut duplicated = Vec::new();
    for (verb, _) in VERB_USAGE {
        if !seen.insert(*verb) {
            duplicated.push(*verb);
        }
    }
    assert!(
        duplicated.is_empty(),
        "a duplicate verb is unreachable text: `usage_for` returns the first \
         match, so every entry after it can never be printed: {duplicated:?}"
    );
}

/// The lookup must answer for every verb the table declares, which is what
/// makes a table entry reachable in the first place. A verb present in the
/// table but absent from `usage_for` would be a second, different way to write
/// unreachable help text.
#[test]
fn a_verb_in_the_table_resolves_through_the_lookup() {
    for (verb, usage) in VERB_USAGE {
        assert_eq!(
            usage_for(verb),
            Some((*usage).to_owned()),
            "{verb} is in the table but the lookup does not return its text"
        );
    }
}

/// No verb left in the table has a record: `usage_for` reads the registry
/// first, so a row left behind after a command moved into a record is text no
/// command can print, and the table would be a second source for one verb.
#[test]
fn every_verb_left_in_the_table_has_no_record() {
    for (verb, _) in VERB_USAGE {
        assert!(
            crate::cli_command_registry::find(verb).is_none(),
            "`{verb}` has a registry record and a row in the usage table"
        );
    }
}

/// Every registered command answers through its registry record, which is what
/// makes a row's deletion byte-safe: the lookup reads the record, and the record
/// carries the text the row held.
///
/// The independent input here is the lookup's dispatch: registered verbs have
/// no fallback row in the table (checked above), so this assertion fails if
/// `usage_for` or `usage_line` stops reading records, and the bytes themselves
/// are pinned by the per-record tests and the frozen fixture.
#[test]
fn a_registered_command_resolves_through_the_lookup() {
    for spec in crate::cli_command_registry::all() {
        assert_eq!(
            usage_for(spec.name),
            Some(spec.full_usage()),
            "{} is registered but the lookup does not return its record text",
            spec.name
        );
        assert_eq!(
            cli_help_usage::usage_line(spec.name),
            Some(spec.usage_line()),
            "{} is registered but its usage line does not resolve",
            spec.name
        );
    }
}

/// The consolidate selector was renamed to `bank` with no alias, so the usage
/// line must keep advertising `--bank` and must never advertise `scope` again:
/// a caller reading stale help would send a flag the parser rejects with a
/// usage error, and a dropped selector would fall back to the session's bank.
#[test]
fn the_consolidate_usage_line_advertises_bank_and_never_scope() {
    let usage = usage_for("consolidate").expect("consolidate has a usage line");
    assert!(usage.contains("[--bank BANK]"), "{usage}");
    assert!(usage.contains("[--force]"), "{usage}");
    assert!(!usage.contains("scope"), "{usage}");
}

/// The same pin over the catalog block `clio help` prints, which is the second
/// surface a reader meets for the same flags.
#[test]
fn the_consolidate_catalog_line_advertises_bank_and_never_scope() {
    let block = crate::help_flags_test_support::entry_block(
        &crate::command_catalog::catalog_text(),
        "consolidate",
    );
    assert!(block.contains("[--bank BANK]"), "{block}");
    assert!(block.contains("[--force]"), "{block}");
    assert!(!block.contains("scope"), "{block}");
}

/// Every flag a command's help text names is one its parser accepts, and every
/// flag the parser accepts is one the command's record names.
///
/// The first direction is the one a reader cannot recover from: help text that
/// names a flag the parser would refuse makes the reader type something that
/// does not exist. The second catches the opposite - a flag the parser takes
/// that nothing names, so it can only be found by reading the parser.
///
/// A registered record splits the check. Its help text may be a hand-written
/// override, so the first direction is real there; the second is true by
/// construction, because `parser_spec` is built from `documented` plus
/// `accepted_extra`, and is asserted as a guard that the derivation stays
/// intact.
///
/// Both contributor counts are pinned apart, never summed into one number:
/// every registered command is decided through its record, and no top-level
/// verb is decided through an authored row. A command that loses its record
/// or its parser moves the first count; a verb that gains an authored row
/// without a record moves the second. Either change fails here instead of
/// hiding inside a total that still adds up.
#[test]
fn help_text_and_parser_flags_agree_in_both_directions() {
    let mut registered_checked = 0usize;
    let mut fallback_checked = 0usize;
    let mut undecidable: Vec<&str> = Vec::new();
    let mut wrong: Vec<String> = Vec::new();

    // Registered commands: the parser the dispatcher resolves for the verb,
    // and the help text the record renders.
    for spec in crate::cli_command_registry::all() {
        let Some(parser) = parser_for(spec.name) else {
            undecidable.push(spec.name);
            continue;
        };
        let accepted = parsed_flag_names(&parser);
        for flag in flags_in(&spec.full_usage()) {
            if !accepted.contains(&flag) {
                wrong.push(format!(
                    "`clio {}` help names {flag}, which its parser refuses",
                    spec.name
                ));
            }
        }
        let named: Vec<&str> = spec
            .documented
            .iter()
            .map(|flag| flag.long)
            .chain(spec.accepted_extra.iter().copied())
            .collect();
        // The record's own names against the parser: a documented flag a
        // hand-written usage override hides must still be one the parser takes.
        for name in &named {
            if !accepted.contains(&format!("--{name}")) {
                wrong.push(format!(
                    "`clio {}` names --{name}, which its parser refuses",
                    spec.name
                ));
            }
        }
        for flag in &accepted {
            if !named.contains(&flag.trim_start_matches("--")) {
                wrong.push(format!(
                    "`clio {}` parser accepts {flag}, which the record does not name",
                    spec.name
                ));
            }
        }
        registered_checked += 1;
    }

    // Verbs with no record are group prefixes (`task`, `sync`), not commands:
    // they hold no usage row, so there is no authored text to compare with a
    // parser. A verb that gains a row without a record is decided here and
    // moves the fallback count below; a row-less prefix states its reason and
    // contributes to neither count.
    for verb in cli_help::READ_VERBS
        .iter()
        .chain(cli_help::WRITE_VERBS.iter())
        .copied()
    {
        if crate::cli_command_registry::find(verb).is_some() {
            continue;
        }
        let Some(usage) = cli_help_usage::usage_line(verb) else {
            continue;
        };
        let Some(parser) = parser_for(verb) else {
            undecidable.push(verb);
            continue;
        };
        let accepted = parsed_flag_names(&parser);
        for flag in flags_in(&usage) {
            if !accepted.contains(&flag) {
                wrong.push(format!(
                    "`clio {verb}` usage names {flag}, which its parser refuses"
                ));
            }
        }
        fallback_checked += 1;
    }

    assert!(
        wrong.is_empty(),
        "help text and parser flag sets disagree:\n{}",
        wrong.join("\n")
    );
    assert!(
        undecidable.is_empty(),
        "these commands have no parser to check against: {undecidable:?}"
    );
    assert_eq!(
        registered_checked, 52,
        "the registered commands this check decides changed, so it is covering less or more \
         than it was: undecidable were {undecidable:?}"
    );
    assert_eq!(
        fallback_checked, 0,
        "a verb without a record gained an authored row, so the fallback branch decides \
         it now: re-pin this count only when that row is intended"
    );
}

/// The flag spec the dispatcher actually parses `verb` with, read side first.
fn parser_for(verb: &str) -> Option<crate::cli_args::FlagSpec> {
    crate::cli_read::spec_for(verb).or_else(|| crate::cli_write::spec_for(verb))
}

/// Every `--name` a parser spec accepts, sorted.
fn parsed_flag_names(spec: &crate::cli_args::FlagSpec) -> Vec<String> {
    let mut names: Vec<String> = spec
        .value_flags()
        .iter()
        .chain(spec.bool_flags().iter())
        .map(|name| format!("--{name}"))
        .collect();
    names.sort();
    names
}

/// The transfer verbs read their key from the recorded store or the file
/// `CLIO_SYNC_KEY_FILE` names, so their usage lines must name those routes and
/// must never advertise the removed `--sync-key` flag or the removed
/// `CLIO_SYNC_KEY` environment rung.
#[test]
fn the_transfer_usage_lines_never_advertise_a_removed_key_route() {
    for verb in ["sync push", "sync pull"] {
        let usage = usage_for(verb).expect("a usage line is registered");
        assert!(usage.contains("CLIO_SYNC_KEY_FILE"), "{verb}: {usage}");
        assert!(usage.contains("clio sync keygen"), "{verb}: {usage}");
        assert!(!usage.contains("--sync-key"), "{verb}: {usage}");
        assert!(!usage.contains("deprecated"), "{verb}: {usage}");
    }
}
