// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the command record model: the usage
//! renderer's hardest shapes and the record-to-parser mapping.
//!
//! ## Owns
//! - Byte-equality tests for `usage_line` and `full_usage` over pure records
//! - The metadata-only pin: flag kind and default never change `parser_spec`
//! - `required_flags` over pure records
//!
//! ## Does not own
//! - The real registry's records (`cli_command_registry_core_reads_tests`)
//! - The registry lookups and validation (`cli_command_registry_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the record model's rendering and parser mapping.

use super::{CommandSpec, Example, Flag, Positional, Section, ValueKind, ValueSpec};

/// The required `ID` positional, as `get` and `validate` carry it.
const REQUIRED_ID: Positional = Positional {
    label: "ID",
    required: true,
    description: "",
};

/// A required `--bank BANK`, the bare first occurrence of a doubled flag.
const REQUIRED_BANK: Flag = Flag {
    required: true,
    ..Flag::value("bank", "BANK")
};

/// `--snapshot`, one arm of the snapshot/gist choice.
const GROUP_SNAPSHOT: Flag = Flag {
    exclusive_group: Some("snapshot-gist"),
    ..Flag::boolean("snapshot")
};

/// `--gist`, the other arm of the snapshot/gist choice.
const GROUP_GIST: Flag = Flag {
    exclusive_group: Some("snapshot-gist"),
    ..Flag::boolean("gist")
};

/// A pure command record with the fields these tests do not vary left empty.
const fn record(
    name: &'static str,
    positionals: &'static [Positional],
    documented: &'static [Flag],
    notes: &'static [&'static str],
    usage_alternates: &'static [&'static str],
    usage_override: Option<&'static str>,
) -> CommandSpec {
    CommandSpec {
        name,
        aliases: &[],
        summary: "",
        notes,
        positionals,
        documented,
        accepted_extra: &[],
        flag_aliases: &[],
        usage_override,
        usage_alternates,
        examples: &[],
        tool: None,
        section: Section::Read,
    }
}

/// An optional value keeps its own bracket, so a reader can tell that the value
/// may be omitted without confusing it with the flag's own optional bracket.
#[test]
fn an_optional_value_keeps_its_own_bracket() {
    const FLAGS: &[Flag] = &[Flag {
        value_optional: true,
        ..Flag::value("bind", "ADDR")
    }];
    let spec = record("mcp http", &[], FLAGS, &[], &[], None);
    assert_eq!(spec.usage_line(), "clio mcp http [--bind [ADDR]]");
    assert_eq!(spec.full_usage(), "usage: clio mcp http [--bind [ADDR]]");
}

/// A repeatable flag carries the ellipsis after the closing bracket, because the
/// bracket belongs to the flag and the ellipsis to the repetition.
#[test]
fn a_repeatable_flag_carries_the_ellipsis_after_its_bracket() {
    const FLAGS: &[Flag] = &[Flag {
        repeatable: true,
        ..Flag::value("database-option", "NAME=VALUE")
    }];
    let spec = record("setup", &[], FLAGS, &[], &[], None);
    assert_eq!(
        spec.usage_line(),
        "clio setup [--database-option NAME=VALUE]..."
    );
}

/// One command may document the same flag twice; `auth token add` prints a
/// required `--bank` and then an optional repeated `--bank`.
#[test]
fn a_flag_documented_twice_prints_both_occurrences() {
    const FLAGS: &[Flag] = &[REQUIRED_BANK, Flag::value("bank", "BANK")];
    let spec = record("auth token add", &[], FLAGS, &[], &[], None);
    assert_eq!(
        spec.usage_line(),
        "clio auth token add --bank BANK [--bank BANK]"
    );
}

/// Consecutive flags in one exclusive group print as a single alternation token,
/// which is how `get` shows that one of `--snapshot` or `--gist` is chosen.
#[test]
fn an_exclusive_group_prints_as_one_alternation_token() {
    const FLAGS: &[Flag] = &[
        GROUP_SNAPSHOT,
        GROUP_GIST,
        Flag::value("output", "json|text"),
    ];
    let spec = record("get", &[REQUIRED_ID], FLAGS, &[], &[], None);
    assert_eq!(
        spec.usage_line(),
        "clio get ID [--snapshot|--gist] [--output json|text]"
    );
}

/// A required positional prints bare; an optional one prints in brackets.
#[test]
fn a_required_positional_prints_bare_and_an_optional_one_in_brackets() {
    const REQUIRED: &[Positional] = &[Positional {
        label: "QUERY",
        required: true,
        description: "",
    }];
    const OPTIONAL: &[Positional] = &[Positional {
        label: "QUERY",
        required: false,
        description: "",
    }];
    let required = record("recall", REQUIRED, &[], &[], &[], None);
    let optional = record("compose-context", OPTIONAL, &[], &[], &[], None);
    assert_eq!(required.usage_line(), "clio recall QUERY");
    assert_eq!(optional.usage_line(), "clio compose-context [QUERY]");
}

/// A required value flag prints without brackets; nothing about it is optional.
#[test]
fn a_required_value_flag_prints_without_brackets() {
    const FLAGS: &[Flag] = &[Flag {
        required: true,
        ..Flag::value("action", "attest|update|invalidate|delete")
    }];
    let spec = record("validate", &[REQUIRED_ID], FLAGS, &[], &[], None);
    assert_eq!(
        spec.usage_line(),
        "clio validate ID --action attest|update|invalidate|delete"
    );
}

/// A usage override replaces the assembled line, and the notes still print
/// under it; the undocumented flag proves the assembled tokens were dropped.
#[test]
fn a_usage_override_replaces_the_line_and_keeps_the_notes() {
    const FLAGS: &[Flag] = &[Flag::boolean("confirm")];
    const OVERRIDE: &str =
        "clio backup verify ID | clio backup verify --latest [--output json|text]";
    let spec = record(
        "backup verify",
        &[],
        FLAGS,
        &["a note"],
        &[],
        Some(OVERRIDE),
    );
    assert_eq!(spec.usage_line(), OVERRIDE);
    assert_eq!(
        spec.full_usage(),
        format!("usage: {OVERRIDE}\n       a note")
    );
}

/// Each alternate prints on its own `or:` line under the usage line, and the
/// notes follow the alternates, indented by exactly seven spaces.
#[test]
fn an_alternate_follows_the_line_and_notes_follow_the_alternates() {
    const FLAGS: &[Flag] = &[Flag {
        required: true,
        ..Flag::value("file", "ops.json")
    }];
    const ALTERNATES: &[&str] = &["clio batch --file - (reads operations JSON from stdin)"];
    let spec = record("batch", &[], FLAGS, &["a note"], ALTERNATES, None);
    let text = spec.full_usage();
    assert_eq!(
        text,
        "usage: clio batch --file ops.json\n       or: clio batch --file - (reads operations JSON from stdin)\n       a note"
    );
    let alternate_at = text.find("or: clio").expect("the alternate prints");
    let note_at = text.find("a note").expect("the note prints");
    assert!(alternate_at < note_at, "{text}");
}

/// Flag kind, default, description, short spelling, placeholder override,
/// optional-value marker, exclusivity, the record's section, examples,
/// alternate, and override text are documentation metadata: none of them may
/// reach the parser spec, or help text could change what the parser accepts.
#[test]
fn parser_spec_ignores_flag_metadata() {
    const PLAIN: &[Flag] = &[
        Flag::value("limit", "N"),
        Flag::value("min-weight", "W"),
        Flag::value("min-score", "S"),
        GROUP_SNAPSHOT,
        GROUP_GIST,
    ];
    const DECORATED: &[Flag] = &[
        Flag {
            short: Some('l'),
            default: Some("10"),
            description: "page size",
            placeholder_override: Some("COUNT"),
            value_optional: true,
            exclusive_group: Some("limit-choice"),
            value: Some(ValueSpec {
                placeholder: "N",
                kind: ValueKind::Uint,
            }),
            ..Flag::value("limit", "N")
        },
        Flag {
            value: Some(ValueSpec {
                placeholder: "W",
                kind: ValueKind::Float,
            }),
            ..Flag::value("min-weight", "W")
        },
        Flag {
            value: Some(ValueSpec {
                placeholder: "S",
                kind: ValueKind::Enum(&["10", "20"]),
            }),
            ..Flag::value("min-score", "S")
        },
        GROUP_SNAPSHOT,
        GROUP_GIST,
    ];
    const EXAMPLES: &[Example] = &[Example {
        command: "clio get ID --limit 10",
        comment: Some("one item"),
    }];
    let plain = record("get", &[REQUIRED_ID], PLAIN, &[], &[], None);
    let decorated = CommandSpec {
        examples: EXAMPLES,
        usage_override: Some("clio get ID --limit 10"),
        usage_alternates: &["clio get ID"],
        section: Section::Write,
        ..record("get", &[REQUIRED_ID], DECORATED, &[], &[], None)
    };
    let reserved = CommandSpec {
        section: Section::Reserved,
        ..record("get", &[REQUIRED_ID], PLAIN, &[], &[], None)
    };
    assert_ne!(
        plain.usage_line(),
        decorated.usage_line(),
        "the records must differ in metadata for this check to mean anything"
    );
    assert_eq!(
        decorated.parser_spec().value_flags(),
        plain.parser_spec().value_flags()
    );
    assert_eq!(
        decorated.parser_spec().bool_flags(),
        plain.parser_spec().bool_flags()
    );
    assert_eq!(reserved.usage_line(), plain.usage_line());
    assert_eq!(
        reserved.parser_spec().value_flags(),
        plain.parser_spec().value_flags()
    );
}

/// `accepted_extra` must name a declared global flag, so the record fails
/// closed: an undeclared name would silently parse a flag the usage line never
/// mentions, and `parser_spec` panics instead of allowing that.
#[test]
#[should_panic(expected = "accepted_extra names a declared global flag")]
fn parser_spec_panics_when_accepted_extra_names_an_undeclared_flag() {
    let undeclared = CommandSpec {
        accepted_extra: &["not-a-global-flag"],
        ..record("get", &[REQUIRED_ID], &[], &[], &[], None)
    };
    let _ = undeclared.parser_spec();
}

/// `required_flags` names every documented required flag, in print order.
#[test]
fn required_flags_lists_the_required_documented_flags_in_order() {
    const FLAGS: &[Flag] = &[
        Flag {
            required: true,
            ..Flag::value("label", "LABEL")
        },
        Flag::boolean("dry-run"),
        Flag {
            required: true,
            ..Flag::value("reason", "R")
        },
    ];
    let spec = record("discard", &[REQUIRED_ID], FLAGS, &[], &[], None);
    assert_eq!(spec.required_flags(), ["label", "reason"]);
}
