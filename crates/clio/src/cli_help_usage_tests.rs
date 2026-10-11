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
//! - The no-duplicate-verb invariant, and the "a looked-up verb resolves to its
//!   own text" check
//! - The usage-line-versus-parser flag agreement pin: every flag a usage line
//!   names is one its verb's parser accepts, pinned per verb so a verb that
//!   loses its spec or its usage line fails
//! - The never-advertise-a-removed-route pin: the transfer usage lines name the
//!   store and `CLIO_SYNC_KEY_FILE` routes and never advertise `--sync-key` or
//!   the removed `CLIO_SYNC_KEY` environment rung
//! - The consolidate rename pin: the two help strings the rename touched keep
//!   advertising `--bank` and never advertise the removed `scope` again
//!
//! ## Does not own
//! - The wording of any other usage line
//! - The help catalog and tool bindings (`cli_help_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use std::collections::BTreeSet;

use super::{VERB_USAGE, usage_for};
use crate::cli_args;
use crate::cli_help;
use crate::cli_help_usage;
use crate::help_flags_test_support::flags_in;

/// Regression cover for issue #84: `VERB_USAGE` held two entries keyed
/// `backup restore` and `usage_for` returns the first match, so the second was
/// text no command, flag, or output mode could print. A maintainer who rewrote
/// it saw no change and no diagnostic. `usage_for` has no way to tell two
/// variants of one verb apart, so the table must hold one entry per verb.
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
            Some(*usage),
            "{verb} is in the table but the lookup does not return its text"
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

#[test]
fn correct_usage_and_catalog_name_the_confirm_flag() {
    let usage = usage_for("correct").expect("correct has a usage line");
    assert!(usage.contains("[--confirm]"), "{usage}");

    let block = crate::help_flags_test_support::entry_block(
        &crate::command_catalog::catalog_text(),
        "correct",
    );
    assert!(block.contains("[--confirm]"), "{block}");
}

/// A flag a usage line names is one the verb's parser will accept.
///
/// The other direction is checked elsewhere: a flag in `--help` reaches the catalog.
/// This is the direction that catches the defect a reader cannot recover from — help
/// text naming a flag the parser would refuse, so the reader types it and is told it
/// does not exist. The reported instance was `am compose down`, a help line naming a
/// binary that is not installed.
///
/// Only decidable for the verbs that share a flag spec. The reserved surfaces read
/// their flags through a name-to-value map that accepts anything, so there is nothing
/// to check them against and they are counted separately rather than passed over.
#[test]
fn a_flag_a_usage_line_names_is_one_the_parser_accepts() {
    let mut checked = 0usize;
    let mut undecidable: Vec<&str> = Vec::new();
    let mut wrong: Vec<String> = Vec::new();
    for verb in cli_help::READ_VERBS
        .iter()
        .chain(cli_help::WRITE_VERBS.iter())
        .copied()
    {
        let Some(usage) = cli_help_usage::usage_line(verb) else {
            continue;
        };
        let spec = crate::cli_read::spec_for(verb).or_else(|| crate::cli_write::spec_for(verb));
        let Some(spec) = spec else {
            undecidable.push(verb);
            continue;
        };
        let mut accepted: Vec<String> = spec
            .value_flags()
            .iter()
            .chain(spec.bool_flags().iter())
            .chain(cli_args::GLOBAL_VALUE_FLAGS.iter())
            .chain(cli_args::GLOBAL_BOOL_FLAGS.iter())
            .map(|name| format!("--{name}"))
            .collect();
        accepted.sort();
        for flag in flags_in(usage) {
            if !accepted.contains(&flag) {
                wrong.push(format!(
                    "`clio {verb}` names {flag}, which its parser refuses"
                ));
            }
        }
        checked += 1;
    }
    assert!(
        wrong.is_empty(),
        "help text names flags the parser would reject:\n{}",
        wrong.join("\n")
    );
    // Pinned rather than a floor, so a verb that loses its spec or its usage line
    // shows up as a number that changed instead of as coverage that quietly shrank.
    assert_eq!(
        checked, 20,
        "the number of verbs this check decides changed, so it is covering less or more \
         than it was: undecidable were {undecidable:?}"
    );
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
