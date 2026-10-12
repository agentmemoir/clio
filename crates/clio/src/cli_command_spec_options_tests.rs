// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the options block the record model
//! renders: flag descriptions, real defaults, placeholder overrides, short
//! spellings, and the documented-versus-extra grouping.
//!
//! ## Owns
//! - Byte-level tests for `options_help` and the lines `flag_help_line` writes
//! - The fail-closed pin: an undeclared `accepted_extra` name panics the render
//!
//! ## Does not own
//! - The usage-line and alternates rendering (`cli_command_spec_tests`)
//! - The real registry's records and their text (`cli_command_registry_*_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the options block's own rendering.

use crate::cli_command_spec::{CommandSpec, Flag, Section};

/// A pure command record with the fields these tests do not vary left empty.
const fn record(
    documented: &'static [Flag],
    accepted_extra: &'static [&'static str],
    notes: &'static [&'static str],
) -> CommandSpec {
    CommandSpec {
        name: "recall",
        aliases: &[],
        summary: "",
        notes,
        positionals: &[],
        documented,
        accepted_extra,
        flag_aliases: &[],
        usage_override: None,
        usage_alternates: &[],
        examples: &[],
        tool: None,
        section: Section::Read,
    }
}

/// A value flag whose help line carries a description and a real default.
const LIMIT: Flag = Flag {
    default: Some("10"),
    ..Flag::uint("limit", "N", "Caps the result page.")
};

/// An option line prints `--long VALUE  description [default: X]`, the
/// description and the default suffix each only when the record carries them.
#[test]
fn an_option_line_carries_its_description_and_default() {
    const FLAGS: &[Flag] = &[LIMIT, Flag::toggle("entities", "Resolves entity mentions.")];
    let spec = record(FLAGS, &[], &[]);
    assert_eq!(
        spec.options_help(),
        "Options:\n  --limit N  Caps the result page. [default: 10]\n  \
         --entities  Resolves entity mentions."
    );
}

/// A blank description never prints as an empty gap: the flag token and the
/// default suffix still name the flag and its value.
#[test]
fn a_blank_description_prints_only_the_flag_token_and_its_default() {
    const FLAGS: &[Flag] = &[Flag {
        default: Some("7"),
        ..Flag::value("offset", "N")
    }];
    let spec = record(FLAGS, &[], &[]);
    assert_eq!(spec.options_help(), "Options:\n  --offset N [default: 7]");
}

/// The placeholder override replaces the value token on the help line, which
/// is the approved rendering for a flag whose value needs a clearer name.
#[test]
fn the_placeholder_override_replaces_the_value_token() {
    const FLAGS: &[Flag] = &[Flag {
        placeholder_override: Some("COUNT"),
        ..LIMIT
    }];
    let spec = record(FLAGS, &[], &[]);
    assert!(
        spec.options_help().contains("  --limit COUNT  "),
        "{}",
        spec.options_help()
    );
    assert!(
        !spec.options_help().contains("--limit N "),
        "{}",
        spec.options_help()
    );
}

/// A value the caller may omit keeps its own bracket on the help line, the
/// same shape the usage line gives it.
#[test]
fn an_optional_value_keeps_its_bracket_on_the_option_line() {
    const FLAGS: &[Flag] = &[Flag {
        value_optional: true,
        ..Flag::value("bind", "ADDR")
    }];
    let spec = record(FLAGS, &[], &[]);
    assert_eq!(spec.options_help(), "Options:\n  --bind [ADDR]");
}

/// The record's own flags print under `Options:`, and the globals it accepts
/// without documenting print under `Global flags:`, each rendered from its
/// shared `GLOBAL_*` definition so every command describes it the same way.
#[test]
fn documented_and_extra_flags_print_in_separate_groups() {
    const FLAGS: &[Flag] = &[LIMIT];
    const EXTRAS: &[&str] = &["db", "yes"];
    let spec = record(FLAGS, EXTRAS, &[]);
    let options = spec.options_help();
    assert_eq!(
        options,
        format!(
            "Options:\n  --limit N  Caps the result page. [default: 10]\nGlobal flags:\n{}\n{}",
            CommandSpec::flag_help_line(
                crate::cli_command_registry::global_flag("db").expect("db is a declared global")
            ),
            CommandSpec::flag_help_line(
                crate::cli_command_registry::global_flag("yes").expect("yes is a declared global")
            )
        ),
        "{options}"
    );
    let extras_at = options
        .find("Global flags:")
        .expect("the extra group prints");
    let documented_at = options
        .find("--limit")
        .expect("the documented group prints");
    assert!(documented_at < extras_at, "{options}");
}

/// A record that documents no flag of its own prints only the shared globals
/// group, so a heading never stands over an empty list.
#[test]
fn only_the_extras_group_prints_when_nothing_is_documented() {
    const FLAGS: &[Flag] = &[];
    const EXTRAS: &[&str] = &["db"];
    let spec = record(FLAGS, EXTRAS, &[]);
    let options = spec.options_help();
    assert!(options.starts_with("Global flags:"), "{options}");
    assert!(!options.contains("Options:"), "{options}");
}

/// A record with neither documented flags nor accepted extras renders no
/// options block at all, and its `full_usage` gains no heading.
#[test]
fn a_block_with_nothing_to_show_prints_nothing() {
    let spec = record(&[], &[], &[]);
    assert_eq!(spec.options_help(), "");
    assert_eq!(spec.full_usage(), "usage: clio recall");
    assert!(
        !spec.full_usage().contains("Options:"),
        "{}",
        spec.full_usage()
    );
}

/// The block a record with no notes renders is flags only: the note prose
/// stays in `full_usage`, and the appended block adds nothing but the flags.
#[test]
fn the_options_block_carries_only_flag_lines() {
    const FLAGS: &[Flag] = &[LIMIT];
    const NOTES: &[&str] = &["a score floor is a relative filter"];
    let spec = record(FLAGS, &[], NOTES);
    let options = spec.options_help();
    assert_eq!(
        options,
        "Options:\n  --limit N  Caps the result page. [default: 10]"
    );
}

/// An `accepted_extra` name that is not a declared global fails the render,
/// the same fail-closed rule `parser_spec` applies to the same list.
#[test]
#[should_panic(expected = "accepted_extra names a declared global flag")]
fn options_help_panics_when_accepted_extra_names_an_undeclared_flag() {
    let spec = record(&[], &["not-a-global-flag"], &[]);
    let _ = spec.options_help();
}
