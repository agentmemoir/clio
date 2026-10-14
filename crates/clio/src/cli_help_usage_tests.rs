// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for pinning the usage lines the registry records
//! carry: that every registered command resolves through the lookup, that a
//! command's help text and its parser agree on flags in both directions, and
//! that no line a reader sees advertises a removed route.
//!
//! ## Owns
//! - The registry-resolution pin: every registered command resolves through
//!   `usage_for` and `usage_line` to its record's own text, so a record the
//!   lookup stops answering fails here rather than printing the catalog's
//!   "no usage line registered" marker
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
//! - The surviving-content pins: the candidate verbs keep their
//!   `--epistemic-kind`/`--source-type` pairing in the line and in the rendered
//!   `--help`, and `correct` and `hygiene clean` keep the flags and the
//!   sentence their help text owes
//!
//! ## Does not own
//! - The wording of any other usage line
//! - The help catalog and tool bindings (`cli_help_tests`)
//! - The literal-read scan over the flag-reading modules (`flag_read_scan_test`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::usage_for;
use crate::cli_help;
use crate::cli_help_usage;
use crate::help_flags_test_support::flags_in;

/// The lookup must answer for every registered command, which is what makes a
/// command's `--help` renderable in the first place. A registered command the
/// lookup misses would print the catalog's "no usage line registered" marker.
#[test]
fn every_registered_command_resolves_through_the_lookup() {
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
/// line must keep advertising `--bank` and must never advertise the removed
/// `scope` spelling again: a caller reading stale help would send a flag the
/// parser rejects with a usage error, and a dropped selector would fall back
/// to the session's bank. The spelling is checked bare because `usage_for`
/// carries only the usage line, alternates, and notes with no flag
/// descriptions, so any `scope` here is the removed selector returning.
#[test]
fn the_consolidate_usage_line_advertises_bank_and_never_scope() {
    let usage = usage_for("consolidate").expect("consolidate has a usage line");
    assert!(usage.contains("[--bank BANK]"), "{usage}");
    assert!(usage.contains("[--force]"), "{usage}");
    assert!(!usage.contains("scope"), "{usage}");
}

#[test]
fn hygiene_keep_help_says_it_applies_the_suggested_action() {
    let usage = usage_for("hygiene clean").expect("hygiene clean has a usage line");
    assert!(
        usage.contains("--action keep applies each candidate's suggested action"),
        "{usage}"
    );
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

/// Every verb that takes `--epistemic-kind` and `--source-type` documents the
/// pairing in its own `--help`: the value is required with belief and refused
/// with fact. The catalog renders the first usage line only, so it shows the flag
/// and the pairing sentence stays in `--help`.
#[test]
fn the_candidate_usage_lines_state_the_source_type_pairing() {
    for verb in [
        "remember",
        "admit",
        "triple add",
        "canonical put",
        "shared store",
    ] {
        let usage = usage_for(verb).expect("a usage line is registered");
        assert!(usage.contains("--epistemic-kind"), "{verb}: {usage}");
        assert!(usage.contains("--source-type"), "{verb}: {usage}");
        for value in ["user_stated", "agent_inferred", "third_party"] {
            assert!(usage.contains(value), "{verb} is missing {value}: {usage}");
        }
    }
}

/// The pairing sentence survives to the surface a user reads: run the real
/// `--help` path, not the registry string the usage-line test reads.
#[test]
fn the_rendered_help_shows_the_pairing_line() {
    let (code, out, err) =
        crate::cli_write_test_support::run_write_resolved("remember", &["--help"]);
    assert_eq!(code, std::process::ExitCode::SUCCESS, "{err}");
    assert!(out.contains("are a pair"), "{out}");
    for value in ["user_stated", "agent_inferred", "third_party"] {
        assert!(out.contains(value), "help is missing {value}: {out}");
    }
}

/// A flag a usage line names is one the verb's parser will accept.
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
/// every registered command is decided through its record — a group verb
/// through its group's parser, a parse-gated reserved leaf (`status`,
/// `install`, `uninstall`, the two `compose` leaves, and the `mcp` and `ops`
/// subcommands) through the record-derived names its real parser gates on
/// plus the globals that rule admits, the record-backed leaf surfaces
/// (`backup`, `sync`, `auth token`, `config`, `ranking`, `retention`,
/// `compliance`) through their record's `parser_spec`, and `help` and `setup`
/// through their record's documented entries — and no top-level verb is
/// decided through an authored row. A command that loses its record or its
/// parser moves the first count; a verb that gains an authored row without a
/// record moves the second. Either change fails here instead of hiding inside
/// a total that still adds up.
///
/// The undecidable list must be empty and is printed even so, separately from
/// the literal-read scan's bounded flag-read exemption list: the transports'
/// hidden `--shared-bank` counts as recorded, because the records carry it in
/// `hidden_accepted_flags`.
#[test]
fn help_text_and_parser_flags_agree_in_both_directions() {
    let mut registered_checked = 0usize;
    let mut fallback_checked = 0usize;
    let mut undecidable: Vec<&str> = Vec::new();
    let mut wrong: Vec<String> = Vec::new();

    // Registered commands: the parser the dispatcher resolves for the verb,
    // and the help text the record renders.
    for spec in crate::cli_command_registry::all() {
        let Some(accepted) = parser_for(spec.name) else {
            undecidable.push(spec.name);
            continue;
        };
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
            .chain(
                crate::cli_command_registry_surface::flags::hidden_accepted_flags(spec.name)
                    .iter()
                    .copied(),
            )
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
            let short = flag.trim_start_matches("--");
            // The group parser unions the globals into every
            // spec, so it accepts flags the leaf refuses at runtime (exit 2)
            // and the record carries no flag entry for by design. Each
            // record's own note is the authority for its refused set, so
            // only those flags are exempt here; any other unnamed flag
            // still fails.
            if !named.contains(&short) && !refused_but_parsed(spec.name).contains(&short) {
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
    for verb in cli_help::known_commands() {
        if crate::cli_command_registry::find(verb).is_some() {
            continue;
        }
        let Some(usage) = cli_help_usage::usage_line(verb) else {
            continue;
        };
        let Some(accepted) = parser_for(verb) else {
            undecidable.push(verb);
            continue;
        };
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
    // The parser-parity undecidable list must be empty: every leaf's parser is
    // decided here. The list is printed even when empty, so a reader sees the
    // bound was checked rather than skipped.
    let mut remaining = undecidable.clone();
    remaining.sort_unstable();
    println!(
        "parser-parity undecidable commands ({}), printed separately from the \
         literal-read scan's bounded flag-read exemption list:",
        remaining.len()
    );
    for name in &remaining {
        println!("  clio {name}");
    }
    assert!(
        remaining.is_empty(),
        "these commands have no decidable parser; extend `parser_for` with the \
         derivation their dispatcher really parses with: {remaining:?}"
    );
    assert_eq!(
        registered_checked, 96,
        "the registered commands this check decides changed, so it is covering less or more \
         than it was: undecidable were {undecidable:?}"
    );
    assert_eq!(
        fallback_checked, 0,
        "a verb without a record gained an authored row, so the fallback branch decides \
         it now: re-pin this count only when that row is intended"
    );
}

/// Flags the group parser accepts but the leaf refuses at runtime (exit 2),
/// so the record carries no flag entry for them by design (the record's own
/// refusal note requires it: help must not advertise refused flags). The authority is each record's
/// own note: `backup list` and `backup verify` refuse `--db`/`--backend`
/// (archive-only reads), `auth token add` refuses `--db`/`--backend` (its
/// own `--actor`/`--bank` stay named and parsed), and `auth token list` and
/// `auth token revoke` refuse all four (one credential file, never a store).
fn refused_but_parsed(verb: &str) -> &'static [&'static str] {
    match verb {
        "backup list" | "backup verify" | "auth token add" => &["db", "backend"],
        "auth token list" | "auth token revoke" => &["db", "backend", "actor", "bank"],
        _ => &[],
    }
}

/// The `--flag` names the dispatcher actually parses `verb` with, sorted and
/// de-duplicated.
///
/// A read/write group verb decides through its group's `FlagSpec`. Every other
/// registered command decides through the dispatch that runs it: the
/// parse-gated reserved surfaces (`version`, `health`, `status`, `install`,
/// `uninstall`, the two `compose` leaves, the `mcp` and `ops` subcommands)
/// through the record-derived names their shared parser gates on plus the
/// declared globals that rule admits, and the record-backed leaf surfaces
/// (`backup`, `sync`, `auth token`, `config`, `ranking`, `retention`) through
/// their record's own `parser_spec`. `help`'s dispatch reads the record's
/// documented boolean alone; `setup`'s wizard parser reads its record's
/// documented entries alone and refuses the globals the other surfaces accept.
/// The shared `--help` spelling is a route, not a data flag, so it appears in
/// no set here, exactly as `flags_in` leaves it out of help text.
fn parser_for(verb: &str) -> Option<Vec<String>> {
    if let Some(spec) = crate::cli_read::spec_for(verb).or_else(|| crate::cli_write::spec_for(verb))
    {
        return Some(parsed_flag_names(&spec));
    }
    let record = crate::cli_command_registry::find(verb)?;
    let mut names: Vec<String> = match record.name {
        "version" | "health" | "status" | "install" | "uninstall" | "compose up"
        | "compose down" | "mcp schema-export" | "mcp stdio" | "mcp http" | "ops diagnose"
        | "ops verify" | "ops doctor" | "ops repair" | "ops reindex" => {
            crate::cli_command_registry_surface::parse_flag_names(verb)
                .into_iter()
                .chain(crate::cli_command_registry::global_value_names())
                .chain(crate::cli_command_registry::global_bool_names())
                .map(|name| format!("--{name}"))
                .collect()
        }
        "help" | "setup" => record
            .documented
            .iter()
            .map(|flag| format!("--{}", flag.long))
            .collect(),
        _ => {
            let spec = record.parser_spec();
            spec.value_flags()
                .iter()
                .chain(spec.bool_flags().iter())
                .map(|name| format!("--{name}"))
                .collect()
        }
    };
    names.sort_unstable();
    names.dedup();
    Some(names)
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
