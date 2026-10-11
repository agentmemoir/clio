// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the required-flag contract the read and write commands
//! share: that a verb's required flags are the ones its own usage line says are
//! required, and that the missing ones are reported all at once rather than one
//! run at a time.
//!
//! ## Owns
//! - That the resolved required set agrees with the usage registry, record-sourced
//!   and table-sourced answers alike
//! - That a moved verb answers from its record while the table stays empty
//! - The bare-flag reading of a usage line and the named exceptions to it
//! - `require_flags`' all-at-once refusal and its pasted hint
//!
//! ## Does not own
//! - The argument checks over parsed arguments (`cli_read_args_spec_tests`)
//! - The registry it is checked against (`cli_help_usage_tests`)
//! - The grammar that produces the parsed arguments (`cli_args_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::{REQUIRED_FLAGS, bare_flags, require_flags, required_flags};
use crate::cli_help_usage;

/// Every verb the parity check covers: the table rows plus every record,
/// including the records whose resolved required set is empty. An empty set is
/// not a pass; a record whose usage line prints a mandated flag must answer for
/// it whether its resolved set is empty or not.
fn every_checked_verb() -> Vec<&'static str> {
    let mut verbs: Vec<&'static str> = REQUIRED_FLAGS.iter().map(|(verb, _)| *verb).collect();
    for record in crate::cli_command_registry::all() {
        verbs.push(record.name);
    }
    verbs
}

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

/// The resolved required set of every verb is the one its own usage line states,
/// whether the answer comes from a record or from a table row.
///
/// This is the test that keeps `require_flags` and the help a reader sees from
/// disagreeing. Twelve per-module constants once held this list; a flag added to a
/// usage line would not have reached them, and the failure that returns is a verb
/// asking for one missing flag per run.
///
/// Every record is iterated, not only the ones whose resolved set is non-empty:
/// a record whose usage line prints a mandated flag while its record marks nothing
/// required is the drift this test exists for.
#[test]
fn every_required_flag_list_is_the_one_its_usage_line_states() {
    // The verb-and-flag pairs where a bare reading of the usage line is not the
    // required set, each with the reason. A new one has to be added here, which is
    // the point: the exception is named rather than assumed.
    //
    // `--confirm` is written bare in every verb that has it because it is the gate
    // on the live run and the reader must see it, but a verb that omits it still
    // does its work: `clio discard --reason X` is a dry run, not a refusal. The
    // confirm gate owns that decision, so this table does not.
    //
    // `compose-context` prints `--budget-tokens N` bare so the reader sees a value
    // the run must have, but the record marks it optional: the workspace group's
    // build refuses a run without a value, not the record's required set.
    //
    // Measured, not guessed: `clio discard` and `clio erase` write confirm bare,
    // while `clio update` and `clio correct` bracket theirs. The row below fails
    // if either of those two usage lines is reworded.
    let excused: &[(&str, &str)] = &[
        ("discard", "confirm"),
        ("erase", "confirm"),
        ("compose-context", "budget-tokens"),
    ];
    let mut differences: Vec<String> = Vec::new();
    for verb in every_checked_verb() {
        let usage = cli_help_usage::usage_line(verb)
            .unwrap_or_else(|| panic!("`clio {verb}` must have a registered usage line"));
        let mut stated: Vec<String> = bare_flags(&usage)
            .iter()
            .map(|name| name.trim_start_matches("--").to_owned())
            .filter(|name| {
                !excused
                    .iter()
                    .any(|(excused_verb, flag)| *excused_verb == verb && flag == name)
            })
            .collect();
        stated.sort();
        let mut listed: Vec<String> = required_flags(verb)
            .into_iter()
            .map(str::to_owned)
            .collect();
        listed.sort();
        if listed != stated {
            differences.push(format!(
                "`clio {verb}`: the resolved list is {listed:?} and its usage line states {stated:?}"
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
    for (verb, excused) in [
        ("discard", "confirm"),
        ("erase", "confirm"),
        ("compose-context", "budget-tokens"),
    ] {
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

/// Every verb the parity check covers has a usage line, so neither source can hold
/// a required set the help surface has never heard of.
#[test]
fn every_covered_verb_has_a_usage_line() {
    for verb in every_checked_verb() {
        assert!(
            cli_help_usage::usage_line(verb).is_some(),
            "`{verb}` carries required flags but has no usage line"
        );
    }
}

/// The table is intentionally empty: every verb this phase moved has a record,
/// so no verb answers from a row. The loop below stays as future-proofing and
/// runs only if a row is added later.
/// A verb without a record still reports its table row unchanged, verbatim.
#[test]
fn a_table_verb_keeps_its_required_flag_set() {
    assert!(
        REQUIRED_FLAGS.is_empty(),
        "the table is intentionally empty while every moved verb has a record"
    );
    // The table is intentionally empty; this loop runs only if a row lands here.
    for &(verb, row) in REQUIRED_FLAGS {
        assert!(
            crate::cli_command_registry::find(verb).is_none(),
            "`{verb}` has a record, so the table row is a second source"
        );
        assert_eq!(required_flags(verb).as_slice(), row, "{verb}");
    }
    assert!(required_flags("version").is_empty(), "no record, no row");
}

/// No row is left for a verb whose record owns its required bits, so every verb
/// still has exactly one source for them.
#[test]
fn no_table_row_remains_for_a_verb_whose_record_owns_its_bits() {
    for record in crate::cli_command_registry::all() {
        if record.required_flags().is_empty() {
            continue;
        }
        assert!(
            !REQUIRED_FLAGS.iter().any(|(verb, _)| *verb == record.name),
            "`{}` has a record, so its row is a second source that can drift",
            record.name
        );
    }
}

/// Each moved verb's required bits are the row it deleted, and its missing-flag
/// refusal is byte-identical to the one that row produced.
///
/// The list `require_flags` receives is the only thing that moved, so this pins
/// both ends: the record answers with the row's flags in the row's order, and the
/// message, hint, and exit code are the pinned originals.
#[test]
fn a_moved_verbs_required_bits_and_refusal_match_its_deleted_row() {
    let cases: &[(&str, &[&str], &str, &str)] = &[
        (
            "task upsert",
            &["definition", "status"],
            "`task upsert` requires --definition, --status (2 missing)",
            "try `clio task upsert ... --definition VALUE --status VALUE`",
        ),
        (
            "failure record",
            &["attempt-n", "what-failed", "lesson"],
            "`failure record` requires --attempt-n, --what-failed, --lesson (3 missing)",
            "try `clio failure record ... --attempt-n VALUE --what-failed VALUE --lesson VALUE`",
        ),
        (
            "triple end",
            &["valid-until"],
            "`triple end` requires --valid-until",
            "try `clio triple end ... --valid-until VALUE`",
        ),
        (
            "belief observe",
            &["confidence", "evidence", "source"],
            "`belief observe` requires --confidence, --evidence, --source (3 missing)",
            "try `clio belief observe ... --confidence VALUE --evidence VALUE --source VALUE`",
        ),
        (
            "graph link",
            &["relationship"],
            "`graph link` requires --relationship",
            "try `clio graph link ... --relationship VALUE`",
        ),
        (
            "canonical put",
            &["category", "epistemic-kind"],
            "`canonical put` requires --category, --epistemic-kind (2 missing)",
            "try `clio canonical put ... --category VALUE --epistemic-kind VALUE`",
        ),
        (
            "update",
            &["rule"],
            "`update` requires --rule",
            "try `clio update ... --rule VALUE`",
        ),
        (
            "discard",
            &["reason"],
            "`discard` requires --reason",
            "try `clio discard ... --reason VALUE`",
        ),
        (
            "correct",
            &["reason"],
            "`correct` requires --reason",
            "try `clio correct ... --reason VALUE`",
        ),
        (
            "export",
            &["out"],
            "`export` requires --out",
            "try `clio export ... --out VALUE`",
        ),
        (
            "import",
            &["source"],
            "`import` requires --source",
            "try `clio import ... --source VALUE`",
        ),
        (
            "erase",
            &["subject", "basis", "request"],
            "`erase` requires --subject, --basis, --request (3 missing)",
            "try `clio erase ... --subject VALUE --basis VALUE --request VALUE`",
        ),
    ];
    let parsed = crate::cli_args::parse(
        &crate::cli_args::FlagSpec::new(&[], &[]),
        Vec::<String>::new(),
    )
    .expect("parse");
    for &(verb, row, message, hint) in cases {
        let record = crate::cli_command_registry::find(verb).expect("the record has arrived");
        assert_eq!(record.required_flags().as_slice(), row, "{verb} record");
        assert_eq!(required_flags(verb).as_slice(), row, "{verb} resolved");
        let resolved = require_flags(&parsed, verb, required_flags(verb)).expect_err("all missing");
        let from_row = require_flags(&parsed, verb, row).expect_err("all missing");
        assert_eq!(resolved.message(), message, "{verb}");
        assert_eq!(resolved.hint(), Some(hint), "{verb}");
        assert_eq!(
            resolved.exit_code(),
            std::process::ExitCode::from(crate::cli_error::EXIT_USAGE),
            "{verb}"
        );
        assert_eq!(resolved.message(), from_row.message(), "{verb}");
        assert_eq!(resolved.hint(), from_row.hint(), "{verb}");
        assert_eq!(resolved.exit_code(), from_row.exit_code(), "{verb}");
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

/// The five core read verbs carry no required flag in their records, so
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

/// The missing-flag refusal for a moved verb is unchanged, message and hint
/// included, now that the record answers for the table row it replaced.
#[test]
fn a_moved_verbs_missing_flag_refusal_is_unchanged() {
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
