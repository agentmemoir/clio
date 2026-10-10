// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the hand-rolled CLI argument grammar: positionals, the
//! `--` end-of-options delimiter, value and boolean flags, both `--flag value`
//! and `--flag=value`, fail-closed unknown flags, and leading-global splitting.
//!
//! ## Owns
//! - Assertions over `parse` and `split_leading_globals`
//!
//! ## Does not own
//! - The parser implementation under test (`cli_args`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;

fn spec() -> FlagSpec {
    FlagSpec::new(&["limit", "domains"], &["snapshot", "gist"])
}

fn tokens(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

#[test]
fn a_repeating_flag_collects_every_occurrence_in_order() {
    let repeated = spec().repeating(&["domains"]);
    let parsed =
        parse(&repeated, tokens(&["--domains", "a,b", "--domains", " c "])).expect("parse");
    assert_eq!(
        parsed.values_of("domains"),
        &["a,b".to_owned(), " c ".to_owned()]
    );
    // A repeated flag is still missing-value checked.
    let err = parse(&repeated, tokens(&["--domains"])).expect_err("missing value");
    assert!(
        err.message().contains("requires a value"),
        "{}",
        err.message()
    );
    // An inline form is collected too.
    let parsed = parse(&repeated, tokens(&["--domains=x"])).expect("parse");
    assert_eq!(parsed.values_of("domains"), &["x".to_owned()]);
    // A non-repeating flag given twice is refused, not last-wins.
    let error = parse(&spec(), tokens(&["--limit", "1", "--limit", "2"])).expect_err("repeat");
    assert!(
        error.message().contains("--limit was given more than once"),
        "{}",
        error.message()
    );
}

#[test]
fn positional_then_flag_keeps_both() {
    let parsed = parse(&spec(), tokens(&["q", "--limit", "2"])).expect("parse");
    assert_eq!(parsed.positional(0), Some("q"));
    assert_eq!(parsed.value("limit"), Some("2"));
}

#[test]
fn boolean_flag_before_positional_does_not_consume_it() {
    let parsed = parse(&spec(), tokens(&["--snapshot", "abc"])).expect("parse");
    assert!(parsed.has("snapshot"));
    assert_eq!(parsed.positional(0), Some("abc"));
}

#[test]
fn equals_syntax_and_globals_before_or_after_the_verb() {
    let parsed = parse(
        &spec(),
        tokens(&["q", "--limit=5", "--bank", "demo", "--output=json"]),
    )
    .expect("parse");
    assert_eq!(parsed.value("limit"), Some("5"));
    assert_eq!(parsed.value("bank"), Some("demo"));
    assert_eq!(parsed.value("output"), Some("json"));
}

#[test]
fn double_dash_makes_every_following_token_positional() {
    let parsed = parse(
        &spec(),
        tokens(&["before", "--", "- note", "--limit=5", "--", "after"]),
    )
    .expect("parse");
    assert_eq!(
        parsed.positionals(),
        tokens(&["before", "- note", "--limit=5", "--", "after"])
    );
}

#[test]
fn unknown_flag_suggests_the_closest_known_flag() {
    let error = parse(&spec(), tokens(&["--limt", "2"])).expect_err("unknown");
    assert_eq!(error.code(), "usage");
    assert_eq!(error.hint(), Some("did you mean `--limit`?"));
}

#[test]
fn unknown_flag_without_a_close_match_has_no_hint() {
    let error = parse(&spec(), tokens(&["--zzzzzz"])).expect_err("unknown");
    assert!(error.hint().is_none());
}

#[test]
fn value_flag_missing_its_value_is_a_usage_error() {
    let error = parse(&spec(), tokens(&["--limit"])).expect_err("missing value");
    assert_eq!(error.code(), "usage");
    assert!(error.hint().is_some());
}

#[test]
fn boolean_flag_rejects_an_inline_value() {
    let error = parse(&spec(), tokens(&["--snapshot=true"])).expect_err("bool value");
    assert_eq!(error.code(), "usage");
    assert!(error.message().contains("takes no value"));
}

#[test]
fn unknown_single_dash_flag_fails_closed() {
    let error = parse(&spec(), tokens(&["-x"])).expect_err("unknown short flag");
    assert_eq!(error.code(), "usage");
}

#[test]
fn help_short_and_long_are_boolean() {
    assert!(parse(&spec(), tokens(&["-h"])).expect("short").has("help"));
    assert!(
        parse(&spec(), tokens(&["--help"]))
            .expect("long")
            .has("help")
    );
}

#[test]
fn short_output_alias_accepts_space_and_equals_forms() {
    let space = parse(&spec(), tokens(&["q", "-o", "json"])).expect("space form");
    assert_eq!(space.value("output"), Some("json"));
    assert_eq!(space.positional(0), Some("q"));

    let joined = parse(&spec(), tokens(&["-o=json", "q"])).expect("equals form");
    assert_eq!(joined.value("output"), Some("json"));
    assert_eq!(joined.positional(0), Some("q"));
}

#[test]
fn short_output_alias_missing_value_is_a_usage_error() {
    let error = parse(&spec(), tokens(&["-o"])).expect_err("missing value");
    assert_eq!(error.code(), "usage");
    assert!(error.hint().is_some());

    // A following flag is not a value: the parser reports the missing value
    // instead of swallowing the next flag.
    let error = parse(&spec(), tokens(&["-o", "--limit", "2"])).expect_err("missing value");
    assert_eq!(error.message(), "flag `-o` requires a value");
}

#[test]
fn attached_short_output_alias_fails_closed() {
    // Only the space and `=` forms mirror `--output`; `-ojson` stays unknown.
    let error = parse(&spec(), tokens(&["-ojson"])).expect_err("attached form");
    assert_eq!(error.code(), "usage");
    assert!(error.message().contains("unknown flag `-ojson`"));
}

#[test]
fn globals_are_part_of_every_spec() {
    let spec = spec();
    assert!(spec.value_flags().contains(&"db"));
    assert!(spec.bool_flags().contains(&"no-input"));
}

#[test]
fn split_leading_globals_moves_flags_after_the_verb() {
    let (rest, globals) = split_leading_globals(&tokens(&["--db", "x.db", "recall", "q"]));
    assert_eq!(rest, tokens(&["recall", "q"]));
    assert_eq!(globals, tokens(&["--db", "x.db"]));
}

#[test]
fn split_leading_globals_handles_inline_and_bool_flags() {
    let (rest, globals) = split_leading_globals(&tokens(&["--db=x.db", "--no-input", "stats"]));
    assert_eq!(rest, tokens(&["stats"]));
    assert_eq!(globals, tokens(&["--db=x.db", "--no-input"]));
}

#[test]
fn split_leading_globals_stops_at_the_end_of_options_delimiter() {
    let (rest, globals) = split_leading_globals(&tokens(&[
        "--db", "x.db", "--", "remember", "--output", "json",
    ]));
    assert_eq!(
        (rest, globals),
        (
            tokens(&["--", "remember", "--output", "json"]),
            tokens(&["--db", "x.db"])
        )
    );
}

#[test]
fn merge_leading_globals_preserves_the_delimiter_as_the_option_boundary() {
    let cases = [
        (
            tokens(&["--category", "fact", "--", "- note"]),
            tokens(&["--db", "x.db", "--output", "json"]),
            tokens(&[
                "--category",
                "fact",
                "--db",
                "x.db",
                "--output",
                "json",
                "--",
                "- note",
            ]),
        ),
        (
            tokens(&["--category", "fact"]),
            tokens(&["--db", "x.db", "--output", "json"]),
            tokens(&["--category", "fact", "--db", "x.db", "--output", "json"]),
        ),
        (
            tokens(&["--", "- note"]),
            tokens(&["--db", "x.db"]),
            tokens(&["--db", "x.db", "--", "- note"]),
        ),
    ];
    for (command_args, globals, expected) in cases {
        assert_eq!(merge_leading_globals(command_args, globals), expected);
    }
}

#[test]
fn split_leading_globals_leaves_trailing_flags_alone() {
    let (rest, globals) = split_leading_globals(&tokens(&["recall", "q", "--db", "x.db"]));
    assert_eq!(rest, tokens(&["recall", "q", "--db", "x.db"]));
    assert!(globals.is_empty(), "{globals:?}");
}

#[test]
fn split_leading_globals_does_not_consume_a_flag_as_a_value() {
    // A value-less `--output` must not swallow the following `--db`; the rest
    // still starts at the verb so the parser reports the missing value.
    let (rest, globals) =
        split_leading_globals(&tokens(&["--output", "--db", "x.db", "recall", "q"]));
    assert_eq!(rest, tokens(&["recall", "q"]));
    assert_eq!(globals, tokens(&["--output", "--db", "x.db"]));
}

#[test]
fn split_leading_globals_moves_the_short_output_alias() {
    let (rest, globals) = split_leading_globals(&tokens(&["-o", "json", "recall", "q"]));
    assert_eq!(rest, tokens(&["recall", "q"]));
    assert_eq!(globals, tokens(&["-o", "json"]));

    let (rest, globals) = split_leading_globals(&tokens(&["-o=json", "recall", "q"]));
    assert_eq!(rest, tokens(&["recall", "q"]));
    assert_eq!(globals, tokens(&["-o=json"]));
}

#[test]
fn split_leading_globals_does_not_consume_a_flag_as_a_short_output_value() {
    // Mirror of the long form: a value-less `-o` must not swallow `--db`.
    let (rest, globals) = split_leading_globals(&tokens(&["-o", "--db", "x.db", "recall", "q"]));
    assert_eq!(rest, tokens(&["recall", "q"]));
    assert_eq!(globals, tokens(&["-o", "--db", "x.db"]));
}

#[test]
fn suggest_returns_the_closest_candidate_only_when_close() {
    assert_eq!(suggest("recal", &["recall", "get"]), Some("recall"));
    assert_eq!(suggest("get", &["recall", "get"]), Some("get"));
    assert_eq!(suggest("zzzzzz", &["recall", "get"]), None);
}

/// The delimiter decides where flags stop, and a boolean flag on the far side of
/// it must not be blamed for a positional that follows.
///
/// This is the one place the parser keeps a positional's index aligned with the
/// boolean flag that came before it, and the alignment is maintained by hand
/// across the branch of the loop that handles `--`. A boolean set before the
/// delimiter belongs to the first token after it and to no later one.
#[test]
fn a_boolean_before_the_delimiter_is_not_carried_past_it() {
    let parsed = parse(&spec(), tokens(&["--snapshot", "--", "first", "second"])).expect("parse");
    assert!(
        parsed.has("snapshot"),
        "the flag was set before the delimiter"
    );
    assert_eq!(
        parsed.bool_before(0),
        Some("snapshot"),
        "the first token after the delimiter is the one the flag precedes"
    );
    assert_eq!(
        parsed.bool_before(1),
        None,
        "a later positional is not preceded by that flag, so blaming it would be wrong"
    );
}

/// The same alignment for a boolean that appears after the delimiter is not
/// possible, because nothing after the delimiter is a flag at all.
#[test]
fn no_token_after_the_delimiter_is_ever_a_boolean_flag() {
    let parsed = parse(&spec(), tokens(&["--", "--snapshot", "x"])).expect("parse");
    assert!(
        !parsed.has("snapshot"),
        "`--snapshot` after the delimiter is a positional, not a flag"
    );
    assert_eq!(parsed.bool_before(0), None);
    assert_eq!(parsed.bool_before(1), None);
}

/// A value flag's value is consumed, so the token after it is not preceded by
/// that value flag and must not be blamed on it.
#[test]
fn a_value_flag_does_not_claim_the_token_after_its_value() {
    let parsed = parse(&spec(), tokens(&["--limit", "5", "extra"])).expect("parse");
    assert_eq!(parsed.value("limit"), Some("5"));
    assert_eq!(
        parsed.bool_before(1),
        None,
        "`--limit` took `5`, so the extra positional is not preceded by a boolean"
    );
}

/// An empty value is refused for every value flag, on the strict parser, by the
/// same rule the reserved surfaces use.
#[test]
fn an_empty_value_is_refused_by_name() {
    for flag in ["--limit", "--limit=", "-o"] {
        let error = parse(&spec(), tokens(&[flag, ""])).expect_err("empty value");
        assert_eq!(error.code(), "usage", "{flag}");
        assert!(
            error.message().contains("must not be empty"),
            "{flag}: {}",
            error.message()
        );
    }
}

/// The global `-o` is the short spelling, and an empty one is refused the same way.
#[test]
fn an_empty_short_output_flag_is_refused() {
    let error = parse(&spec(), tokens(&["-o", ""])).expect_err("empty value");
    assert!(
        error.message().contains("-o must not be empty"),
        "{}",
        error.message()
    );
}
