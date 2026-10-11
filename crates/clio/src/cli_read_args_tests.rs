// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the shared argument checks the read and write commands use:
//! that a verb's required flags are the ones its own usage line says are required,
//! and that a missing one is reported all at once rather than one run at a time.
//!
//! ## Owns
//! - That `REQUIRED_FLAGS` agrees with the usage registry, exceptions included
//! - That a registered verb answers from its record while a table verb answers
//!   from its row
//! - Coverage of `require_flags`, `flag`, `unexpected_extra`, `one_positional`,
//!   `no_positionals` and `validate_choice`
//!
//! ## Does not own
//! - The registry it is checked against (`cli_help_usage_tests`)
//! - The grammar that produces the parsed arguments (`cli_args_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::{REQUIRED_FLAGS, bare_flags, flag, require_flags, required_flags};
use crate::cli_help_usage;

/// The flags a verb's usage line writes bare, with the bracket that opens an
/// optional group removed from consideration.
///
/// `[--a X]` and `[--a X|--b Y]` are both optional, `--flag` and `clio` are not
/// flags at all, and a flag the caller may repeat is still written in brackets.
#[test]
fn a_bare_flag_is_one_the_verb_cannot_run_without() {
    assert_eq!(
        bare_flags("clio failure record --attempt-n N --what-failed W [--dry-run]"),
        vec!["--attempt-n".to_owned(), "--what-failed".to_owned()]
    );
    // An alternation inside one bracket is one optional group, not two optional flags.
    assert_eq!(
        bare_flags("clio get ID [--snapshot|--gist]"),
        Vec::<String>::new()
    );
    // A repeatable flag is still optional.
    assert_eq!(bare_flags("clio x [--tag T]..."), Vec::<String>::new());
    // A required flag inside a bracket is optional, which is what makes the check
    // decidable: the bracket is the only signal the reader has.
    assert_eq!(bare_flags("clio x [--out FILE]"), Vec::<String>::new());
}

/// The table of required flags is the usage lines' own, not a second opinion.
///
/// This is the test that keeps `require_flags` and the help a reader sees from
/// disagreeing. Twelve per-module constants once held this list; a flag added to a
/// usage line would not have reached them, and the failure that returns is a verb
/// asking for one missing flag per run.
#[test]
fn every_required_flag_list_is_the_one_its_usage_line_states() {
    // The cases where a bare reading of the usage line is not the required set,
    // each with the reason. A new one has to be added here, which is the point: the
    // exception is named rather than assumed.
    //
    // `--confirm` is written bare in every verb that has it because it is the gate
    // on the live run and the reader must see it, but a verb that omits it still
    // does its work: `clio discard --reason X` is a dry run, not a refusal. The
    // confirm gate owns that decision, so this table does not.
    //
    // Measured, not guessed: `clio discard` and `clio erase` write it bare, while
    // `clio update` and `clio correct` bracket theirs. So the exception list is two
    // rows, and the row below fails if either of those two usage lines is reworded.
    let excused: &[(&str, &str)] = &[("discard", "confirm"), ("erase", "confirm")];
    let mut differences: Vec<String> = Vec::new();
    for (verb, flags) in REQUIRED_FLAGS {
        let usage = cli_help_usage::usage_line(verb)
            .unwrap_or_else(|| panic!("`clio {verb}` must have a registered usage line"));
        let mut stated: Vec<String> = bare_flags(&usage)
            .iter()
            .map(|name| name.trim_start_matches("--").to_owned())
            .filter(|name| !excused.iter().any(|(_, flag)| flag == name))
            .collect();
        stated.sort();
        let mut listed: Vec<String> = flags.iter().map(|name| (*name).to_owned()).collect();
        listed.sort();
        if listed != stated {
            differences.push(format!(
                "`clio {verb}`: the table lists {listed:?} and its usage line states {stated:?}"
            ));
        }
    }
    assert!(
        differences.is_empty(),
        "these verbs' required flags and their own usage lines disagree. A reader can \
         only act on one of them:\n{}",
        differences.join("\n")
    );
}

/// An excused flag is written bare, so the exception above is a fact about the help
/// text rather than a claim about it.
#[test]
fn an_excused_flag_is_written_bare_in_the_usage_line() {
    for (verb, excused) in [("discard", "confirm"), ("erase", "confirm")] {
        let usage = cli_help_usage::usage_line(verb).expect("registered");
        assert!(
            bare_flags(&usage)
                .iter()
                .any(|flag| flag == &format!("--{excused}")),
            "`clio {verb}` now brackets --{excused}, so the exception above is stale and \
             the row can be deleted"
        );
    }
}

/// A verb in the table has a usage line, and a verb with required flags is in the
/// table, so neither list can hold a row the other has never heard of.
#[test]
fn the_table_names_only_verbs_the_registry_knows() {
    for (verb, _) in REQUIRED_FLAGS {
        assert!(
            cli_help_usage::usage_line(verb).is_some(),
            "`{verb}` is in the required-flags table but has no usage line"
        );
    }
}

/// Every missing flag is named in one refusal, so a caller fixes the command line
/// once rather than one flag per run.
#[test]
fn every_missing_required_flag_is_named_at_once() {
    let parsed = crate::cli_args::parse(
        &crate::cli_args::FlagSpec::new(
            &["rule", "reason", "category", "epistemic-kind"],
            &["dry-run"],
        ),
        ["--rule".to_owned(), "r".to_owned()],
    )
    .expect("parse");
    assert!(require_flags(&parsed, "update", ["rule"]).is_ok());
    let error = require_flags(&parsed, "canonical put", ["category", "epistemic-kind"])
        .expect_err("both are missing");
    assert_eq!(error.code(), "usage");
    let message = error.message().to_owned();
    assert!(message.contains("--category"), "{message}");
    assert!(message.contains("--epistemic-kind"), "{message}");
    assert!(
        message.contains("2 missing"),
        "the count lets a caller see it is finished: {message}"
    );
}

/// A verb with no required flags never refuses.
#[test]
fn a_verb_with_no_required_flags_never_refuses() {
    let parsed = crate::cli_args::parse(
        &crate::cli_args::FlagSpec::new(&[], &[]),
        Vec::<String>::new(),
    )
    .expect("parse");
    assert!(require_flags(&parsed, "version", []).is_ok());
}

/// The hint is a command line the reader can paste, so it carries a placeholder for
/// every flag the verb needs rather than naming one of them.
#[test]
fn a_refusal_hint_is_a_command_line_with_a_placeholder_per_flag() {
    let parsed = crate::cli_args::parse(
        &crate::cli_args::FlagSpec::new(&["attempt-n", "what-failed", "lesson"], &[]),
        Vec::<String>::new(),
    )
    .expect("parse");
    let error = require_flags(
        &parsed,
        "failure record",
        ["attempt-n", "what-failed", "lesson"],
    )
    .expect_err("all three are missing");
    assert_eq!(
        error.hint(),
        Some("try `clio failure record ... --attempt-n VALUE --what-failed VALUE --lesson VALUE`")
    );
    assert!(
        error.message().contains("(3 missing)"),
        "the count tells the caller they are finished: {}",
        error.message()
    );
}

/// A flag the required list already proved is present reads back as its value, and
/// an absent one reads as empty rather than panicking, so a forgotten check becomes
/// a message from the tool rather than a crash.
#[test]
fn a_proved_flag_reads_back_as_its_value() {
    let parsed = crate::cli_args::parse(
        &crate::cli_args::FlagSpec::new(&["rule"], &[]),
        ["--rule".to_owned(), "keep".to_owned()],
    )
    .expect("parse");
    assert_eq!(flag(&parsed, "rule"), "keep");
    assert_eq!(flag(&parsed, "absent"), "");
}

/// The five core read verbs carry no required flag in their records yet, so
/// `required_flags` reports an empty set for each.
#[test]
fn core_read_verbs_have_no_required_flags() {
    for verb in ["recall", "get", "show", "inspect", "stats"] {
        assert!(
            required_flags(verb).is_empty(),
            "`{verb}` must have no required flags"
        );
    }
}

/// A verb without a record still reports its table row unchanged.
#[test]
fn a_table_verb_keeps_its_required_flag_set() {
    assert_eq!(
        required_flags("failure record"),
        ["attempt-n", "what-failed", "lesson"]
    );
    assert_eq!(
        required_flags("canonical put"),
        ["category", "epistemic-kind"]
    );
    assert!(required_flags("version").is_empty(), "no record, no row");
}

/// The missing-flag refusal for a table verb is unchanged, message and hint
/// included.
#[test]
fn a_table_verbs_missing_flag_refusal_is_unchanged() {
    let parsed = crate::cli_args::parse(
        &crate::cli_args::FlagSpec::new(&["attempt-n", "what-failed", "lesson"], &[]),
        Vec::<String>::new(),
    )
    .expect("parse");
    let error = require_flags(&parsed, "failure record", required_flags("failure record"))
        .expect_err("all three are missing");
    assert_eq!(error.code(), "usage");
    assert_eq!(
        error.message(),
        "`failure record` requires --attempt-n, --what-failed, --lesson (3 missing)"
    );
    assert_eq!(
        error.hint(),
        Some("try `clio failure record ... --attempt-n VALUE --what-failed VALUE --lesson VALUE`")
    );
}
