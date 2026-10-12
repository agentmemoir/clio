// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the top level's own argument reading and help: which spellings
//! mean help, what an unknown command says, what a help topic resolves to, and the
//! block every surface prints beneath its flags.
//!
//! ## Owns
//! - Coverage of `wants_help`, `unknown_command`, `help_topic` and `report_usage_error`
//! - Coverage of `parse_flags`, `report_surface_usage_error` and `database_help_block`
//! - Coverage of the two no-flag commands' help and the catalog screen
//!
//! ## Does not own
//! - The command and flag tables it reads (`cli_help_tests`)
//! - The surfaces that print the shared block (`ops_cli_tests` and the rest)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;
use crate::cli_error::CliError;
use std::process::ExitCode;

/// `help` in any spelling asks for help, and nothing else does.
///
/// The three spellings are the ones a reader types, and a fourth one silently
/// treated as a flag is how `clio retention --help` once ran the command.
#[test]
fn help_is_recognised_in_every_spelling_and_nothing_else_is() {
    for spelling in ["help", "--help", "-h"] {
        assert!(
            super::wants_help(&[spelling.to_owned()]),
            "`{spelling}` must ask for help"
        );
    }
    for argv in [
        vec![],
        vec!["recall".into(), "q".into()],
        vec!["--helpful".into()],
    ] {
        assert!(!super::wants_help(&argv), "{argv:?} must not ask for help");
    }
}

/// A global value flag's value is not a help request: `--db help` names a
/// database called `help`, so the command runs instead of printing help.
#[test]
fn a_global_value_flags_value_is_not_a_help_request() {
    // Every declared global value flag, and the short `-o`, drops its value.
    let mut flags: Vec<String> = crate::cli_command_registry::global_value_names()
        .map(str::to_owned)
        .collect();
    flags.push("-o".to_owned());
    for flag in flags {
        assert!(
            !super::wants_help(&[flag.clone(), "help".to_owned()]),
            "`{flag} help` must not ask for help"
        );
        assert!(
            !super::wants_help(&[flag.clone(), "-h".to_owned()]),
            "`{flag} -h` must not ask for help"
        );
        assert!(
            super::wants_help(&[flag, "--help".to_owned()]),
            "an explicit --help after a global value flag still asks"
        );
    }
    // A word that is not a value still asks, and `--helpful` never does.
    assert!(super::wants_help(&["recall".to_owned(), "help".to_owned()]));
    assert!(!super::wants_help(&["--helpful".to_owned()]));
}

/// An unknown command suggests the closest one when there is one, and points at
/// `clio help` when there is not.
#[test]
fn an_unknown_command_suggests_a_neighbour_or_points_at_the_catalog() {
    let close = super::unknown_command("recod");
    assert_eq!(close.code(), "usage");
    assert_eq!(close.hint(), Some("did you mean `clio recall`?"));

    let far = super::unknown_command("zzzzqqqq");
    assert_eq!(far.code(), "usage");
    assert_eq!(far.hint(), Some("try `clio help`"));
}

/// `clio help <topic>` resolves the topic and appends `--help`, so the topic's help
/// is whatever that topic's own `--help` prints.
///
/// A topic the CLI does not know is a usage error rather than the whole catalog:
/// the old behaviour made `clio help frobnicate` indistinguishable from
/// `clio help remember`.
#[test]
fn a_help_topic_resolves_to_that_commands_own_help() {
    // The arguments are what followed `clio help`, so an empty list is the catalog.
    assert_eq!(super::help_topic(&[]).expect("bare"), None);

    let (verb, kept) = super::help_topic(&["recall".into()])
        .expect("known topic")
        .expect("a topic was named");
    assert_eq!(verb, "recall");
    assert_eq!(kept, vec!["--help".to_owned()]);

    // A two-word topic reaches the same place the subcommand's own --help does.
    let (verb, kept) = super::help_topic(&["ops".into(), "diagnose".into()])
        .expect("two-word topic")
        .expect("a topic was named");
    assert_eq!(verb, "ops");
    assert_eq!(kept, vec!["diagnose".to_owned(), "--help".to_owned()]);

    // Three tokens is the deepest the CLI accepts, and it resolves too.
    assert!(
        super::help_topic(&["auth".into(), "token".into(), "add".into()])
            .expect("three-token topic")
            .is_some()
    );

    let unknown = super::help_topic(&["frobnicate".into()]).expect_err("unknown");
    assert_eq!(unknown.code(), "usage");
    assert!(
        unknown.message().contains("frobnicate"),
        "{}",
        unknown.message()
    );
}

/// A second word after an unknown topic says the topic takes one, because the
/// reader typed two and the alternative is guessing which one was meant.
#[test]
fn two_words_after_an_unknown_topic_say_the_topic_takes_one() {
    let error = super::help_topic(&["frobnicate".into(), "extra".into()]).expect_err("unknown");
    assert!(
        error
            .hint()
            .is_some_and(|hint| hint.contains("takes one topic")),
        "{:?}",
        error.hint()
    );
}

/// A global value flag's value is not a topic: it is skipped, so a global flag
/// before or after the verb leaves the catalog or the named topic reachable.
///
/// `main` lifts a leading global out from before the verb and appends it after
/// the verb, so both spellings reach `help_topic` as `--db PATH`. Without the
/// skip the path was read as an unknown topic and the command exited 2.
#[test]
fn a_global_value_flags_value_is_not_a_help_topic() {
    for argv in [
        vec!["--db".to_owned(), "/tmp/led.db".to_owned()],
        vec!["--output".to_owned(), "json".to_owned()],
        vec!["-o".to_owned(), "json".to_owned()],
        vec!["--bank".to_owned(), "b".to_owned()],
    ] {
        assert_eq!(
            super::help_topic(&argv).expect("a global flag is not an error"),
            None,
            "{argv:?} names no topic"
        );
    }

    // The topic after the global value still resolves, and the global value is
    // not counted as a second topic word.
    let (verb, kept) = super::help_topic(&[
        "--db".to_owned(),
        "/tmp/led.db".to_owned(),
        "backup".to_owned(),
    ])
    .expect("known topic")
    .expect("a topic was named");
    assert_eq!(verb, "backup");
    assert_eq!(kept, vec!["--help".to_owned()]);

    let unknown = super::help_topic(&[
        "frobnicate".to_owned(),
        "--db".to_owned(),
        "/tmp/led.db".to_owned(),
    ])
    .expect_err("unknown");
    assert!(
        !unknown
            .hint()
            .is_some_and(|hint| hint.contains("takes one topic")),
        "a global value is not a second topic word: {:?}",
        unknown.hint()
    );
}

/// Every declared global value flag, and the short `-o`, drops its value from
/// the topic scan, whatever the flag is spelled, and a two-word topic after the
/// value still resolves. This is the invariant the fix exists to hold: no
/// global value flag's value is ever read as a topic.
#[test]
fn no_global_value_flags_value_is_read_as_a_topic() {
    let mut flags: Vec<String> = crate::cli_command_registry::global_value_names()
        .map(str::to_owned)
        .collect();
    flags.push("-o".to_owned());
    for flag in flags {
        assert_eq!(
            super::help_topic(&[flag.clone(), "backup".to_owned()])
                .expect("a flag is not an error"),
            None,
            "`{flag} VALUE` names no topic"
        );
        let (verb, kept) = super::help_topic(&[
            flag.clone(),
            "backup".to_owned(),
            "ops".to_owned(),
            "diagnose".to_owned(),
        ])
        .expect("a topic after a global resolves")
        .expect("a topic was named");
        assert_eq!(verb, "ops", "`{flag} VALUE` then a two-word topic");
        assert_eq!(kept, vec!["diagnose".to_owned(), "--help".to_owned()]);
    }
}

/// A usage failure is reported in the mode the raw arguments select, and exits 2.
#[test]
fn a_usage_failure_is_reported_in_the_selected_mode_and_exits_two() {
    let error = CliError::usage("something is wrong");
    assert_eq!(
        super::report_usage_error(&error, &["recall".into(), "--output".into(), "json".into()]),
        ExitCode::from(2)
    );
    assert_eq!(
        super::report_surface_usage_error(&error, &["ops".into(), "diagnose".into()]),
        ExitCode::from(2)
    );
    // An explicit `--output json` makes the envelope JSON on any stream, which is
    // what a caller parsing stdout needs.
    assert_eq!(
        super::report_surface_usage_error(&error, &["--output".into(), "json".into()]),
        ExitCode::from(2)
    );
}

/// The two no-flag commands print a usage line and succeed, which is what `--help`
/// on them used to fail to do.
#[test]
fn the_no_flag_commands_print_help_and_succeed() {
    assert_eq!(super::print_version_help(), ExitCode::SUCCESS);
    assert_eq!(super::print_health_help(), ExitCode::SUCCESS);
}

/// `version` documents no flag of its own, so its record help shows only the
/// shared globals group: the same options block `full_usage` appends, so every
/// help path shows the same flag data.
#[test]
fn the_version_record_help_shows_only_the_shared_globals_group() {
    let spec = crate::cli_command_registry::find("version").expect("`version` is registered");
    let options = spec.options_help();
    assert!(options.starts_with("Global flags:"), "{options}");
    assert!(!options.contains("Options:"), "{options}");
}

/// The database block every surface prints also states the empty-value rule, so a
/// reader learns it from the help they already read.
#[test]
fn the_shared_database_block_states_the_empty_value_rule() {
    let block = super::database_help_block(&|_: &str| None);
    assert!(block.contains("--db"), "{block}");
    assert!(
        block.contains("empty") && block.contains("refused"),
        "the shared block must state that an empty value is refused: {block}"
    );
    // A target that cannot be resolved is shown in place, not swallowed, because
    // help must print even when the configured database is unreachable.
    let broken = database_help_block(&|_: &str| Some("not a url".to_owned()));
    assert!(
        broken.contains("cannot be resolved") || broken.contains("precedence"),
        "{broken}"
    );
}

/// A flag the surface does not declare fails closed with a usage error, the same
/// class the shared parser gives, instead of a value stored and silently ignored.
#[test]
fn an_undeclared_flag_fails_closed_with_a_suggestion() {
    // A near miss names the flag it is close to.
    let error = super::parse_flags(
        ["--confidance".to_owned(), "0.5".to_owned()],
        &["confidence"],
    )
    .expect_err("an undeclared flag must be a usage error");
    assert_eq!(error.code(), "usage");
    assert_eq!(error.hint(), Some("did you mean `--confidence`?"));

    // An unknown short flag is refused too, rather than dropped, and is named as
    // it was typed.
    let short =
        super::parse_flags(["-x".to_owned()], &["confidence"]).expect_err("unknown short flag");
    assert_eq!(short.code(), "usage");
    assert_eq!(short.message(), "unknown flag `-x`");

    // A declared flag is accepted; `--help` and `-h` are accepted everywhere.
    let ok = super::parse_flags(
        ["--confidence".to_owned(), "0.5".to_owned()],
        &["confidence"],
    )
    .expect("a declared flag is accepted");
    assert_eq!(ok.get("confidence").map(String::as_str), Some("0.5"));
    for help in ["--help", "-h"] {
        let flags = super::parse_flags([help.to_owned()], &[]).expect("help is always accepted");
        assert_eq!(flags.get("help").map(String::as_str), Some("true"));
    }
}

/// Every documented global flag is accepted on a reserved surface, whatever that
/// surface declares on its own.
///
/// `split_leading_globals` lifts the globals out from before the verb and
/// `merge_leading_globals` hands them back with the surface's own arguments, so a
/// surface whose `known` list did not name them used to refuse the invocation the
/// top level had just rebuilt: `clio --actor bob status` became exit 2.
#[test]
fn every_documented_global_flag_is_accepted_on_a_reserved_surface() {
    for (flag, value) in [
        ("--db", "x"),
        ("--backend", "sqlite"),
        ("--bank", "b"),
        ("--actor", "a"),
        ("--output", "json"),
    ] {
        let flags = super::parse_flags([flag.to_owned(), value.to_owned()], &[])
            .unwrap_or_else(|error| panic!("{flag} must be accepted: {}", error.message()));
        assert_eq!(
            flags.get(flag.trim_start_matches('-')).map(String::as_str),
            Some(value),
            "{flag}"
        );
    }
    for flag in ["--yes", "--no-input"] {
        let flags = super::parse_flags([flag.to_owned()], &[])
            .unwrap_or_else(|error| panic!("{flag} must be accepted: {}", error.message()));
        assert_eq!(
            flags.get(flag.trim_start_matches('-')).map(String::as_str),
            Some("true"),
            "{flag}"
        );
    }
}

/// A token with no leading dash is not a flag and not a value: these surfaces have
/// no operand slot, so it fails closed rather than being dropped.
#[test]
fn a_stray_positional_is_a_usage_error() {
    let error = super::parse_flags(["stray".to_owned()], &["db"]).expect_err("stray positional");
    assert_eq!(error.code(), "usage");
    assert_eq!(error.message(), "unexpected argument `stray`");
}

/// A reserved surface refuses a value flag given twice and a whitespace-only value,
/// the same way the verb grammar does, instead of keeping the last value silently.
#[test]
fn a_reserved_surface_refuses_a_repeated_or_blank_value_flag() {
    let repeated = super::parse_flags(
        ["--db".to_owned(), "a".to_owned(), "--db=b".to_owned()],
        &[],
    )
    .expect_err("repeated --db");
    assert!(
        repeated.message().contains("--db was given more than once"),
        "{}",
        repeated.message()
    );
    let blank =
        super::parse_flags(["--db".to_owned(), "  ".to_owned()], &[]).expect_err("blank --db");
    assert!(
        blank
            .message()
            .contains("--db must not be empty or whitespace-only"),
        "{}",
        blank.message()
    );
}

/// A bare `--` is the top level's delimiter before positional text: the token
/// itself is accepted and ends flag parsing, and a token after it is refused
/// because these surfaces have no operand slot.
#[test]
fn a_double_dash_ends_flag_parsing() {
    let flags = super::parse_flags(
        ["--db".to_owned(), "x".to_owned(), "--".to_owned()],
        &["db"],
    )
    .expect("the delimiter itself is accepted");
    assert_eq!(flags.get("db").map(String::as_str), Some("x"));

    let error = super::parse_flags(
        [
            "--db".to_owned(),
            "x".to_owned(),
            "--".to_owned(),
            "stray".to_owned(),
        ],
        &["db"],
    )
    .expect_err("a token after the delimiter has no operand slot");
    assert_eq!(error.message(), "unexpected argument `stray`");
}

/// A value flag repeated in separate-token form is refused, the same way the
/// `--flag=value` spelling is, instead of the last value silently winning.
#[test]
fn a_value_flag_repeated_in_separate_tokens_is_a_usage_error() {
    let error = super::parse_flags(
        [
            "--db".to_owned(),
            "a".to_owned(),
            "--db".to_owned(),
            "b".to_owned(),
        ],
        &[],
    )
    .expect_err("a repeated --db is a usage error in either spelling");
    assert!(
        error.message().contains("--db was given more than once"),
        "{}",
        error.message()
    );
}

/// A flag name that reaches the unknown-flag reporter without its dash is
/// rendered with the `--` prefix, so the refusal names the flag the way the
/// reader types it. No production caller hands a bare name over today; the
/// branch is defensive, and this test calls the reporter with one directly.
#[test]
fn a_flag_name_without_a_dash_is_rendered_with_the_double_dash() {
    let error = super::unknown_surface_flag("confidance", &["confidence"]);
    assert_eq!(error.code(), "usage");
    assert_eq!(error.message(), "unknown flag `--confidance`");
    assert_eq!(error.hint(), Some("did you mean `--confidence`?"));
}
