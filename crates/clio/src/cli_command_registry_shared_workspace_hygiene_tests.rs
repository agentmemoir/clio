// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the hygiene records against today's
//! usage table and parser flag sets: byte equality of each record's
//! `full_usage` and `usage_line` — the `hygiene clean` alternate and notes
//! included — the exact `parser_spec` flag sequences, each record's summary,
//! tool, section, and authored example count, and the defaults the consumers
//! apply.
//!
//! ## Owns
//! - The three records' usage and parser pins
//! - The `hygiene clean` alternate and note bytes
//! - The hygiene defaults the consumers apply
//! - The sibling file's record-boundary pin
//!
//! ## Does not own
//! - The shared, intent, compose, and audit records and the tranche-level
//!   checks (`cli_command_registry_shared_workspace_tests`)
//! - The flag metadata contract
//!   (`cli_command_registry_shared_workspace_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the hygiene records.

use crate::cli_command_registry_test_support::record;
use crate::cli_command_spec::Section;

/// `hygiene audit` renders today's usage text and builds today's parser sets.
#[test]
fn hygiene_audit_matches_the_registered_usage_and_parser() {
    let spec = record("hygiene audit");
    assert_eq!(
        spec.full_usage(),
        "usage: clio hygiene audit [--min-score N] [--limit N] [--offset N] [--bank BANK] [--output json|text]"
    );
    assert_eq!(
        spec.parser_spec().value_flags(),
        [
            "db",
            "backend",
            "bank",
            "actor",
            "output",
            "min-score",
            "limit",
            "offset"
        ]
    );
    assert_eq!(spec.parser_spec().bool_flags(), ["no-input", "yes"]);
    assert_eq!(spec.summary, "score stored items and list the weakest");
    assert_eq!(spec.tool, Some("hygiene_audit"));
    assert_eq!(spec.section, Section::Read);
    assert!(spec.positionals.is_empty(), "{:?}", spec.positionals);
}

/// `hygiene clean` is the one record with an alternate form: its printed text
/// is `usage:`, then `or:`, then the note, byte-for-byte.
#[test]
fn hygiene_clean_renders_the_or_alternate_and_the_trailing_note() {
    let spec = record("hygiene clean");
    let text = spec.full_usage();
    assert_eq!(
        text,
        "usage: clio hygiene clean --file candidates.json [--action flag|archive|discard|keep] [--confirm] [--dry-run] [--reason R] [--note N] [--bank BANK] [--output json|text]\n       or: clio hygiene clean --file - (reads the candidates JSON from stdin)\n       without --confirm (or with --dry-run) the call is a zero-write dry-run: counts only\n       --action keep applies each candidate's suggested action, including archive or discard"
    );
    assert_eq!(
        spec.usage_line(),
        "clio hygiene clean --file candidates.json [--action flag|archive|discard|keep] [--confirm] [--dry-run] [--reason R] [--note N] [--bank BANK] [--output json|text]"
    );
    assert_eq!(
        spec.usage_alternates,
        ["clio hygiene clean --file - (reads the candidates JSON from stdin)"]
    );
    assert_eq!(
        spec.notes,
        [
            "without --confirm (or with --dry-run) the call is a zero-write dry-run: counts only",
            "--action keep applies each candidate's suggested action, including archive or discard"
        ]
    );
    let or_at = text.find("\n       or: ").expect("the alternate prints");
    let note_at = text.find("\n       without").expect("the note prints");
    assert!(or_at < note_at, "or before note: {text}");
    assert_eq!(
        spec.required_flags(),
        ["file"],
        "the file flag is the one the line writes bare"
    );
    assert_eq!(
        spec.parser_spec().value_flags(),
        [
            "db", "backend", "bank", "actor", "output", "file", "action", "reason", "note"
        ]
    );
    assert_eq!(
        spec.parser_spec().bool_flags(),
        ["no-input", "yes", "confirm", "dry-run"]
    );
    assert_eq!(
        spec.summary,
        "(or --file - for stdin; without --confirm, or with --dry-run: zero-write dry-run only)"
    );
    assert_eq!(spec.tool, Some("hygiene_clean"));
    assert_eq!(spec.section, Section::Write);
    assert!(spec.positionals.is_empty(), "{:?}", spec.positionals);
}

/// `hygiene log` renders today's usage text and builds today's parser sets.
#[test]
fn hygiene_log_matches_the_registered_usage_and_parser() {
    let spec = record("hygiene log");
    assert_eq!(
        spec.full_usage(),
        "usage: clio hygiene log [--limit N] [--offset N] [--bank BANK] [--output json|text]"
    );
    assert_eq!(
        spec.parser_spec().value_flags(),
        [
            "db", "backend", "bank", "actor", "output", "limit", "offset"
        ]
    );
    assert_eq!(spec.parser_spec().bool_flags(), ["no-input", "yes"]);
    assert_eq!(spec.summary, "what hygiene has already done");
    assert_eq!(spec.tool, Some("hygiene_log_list"));
    assert_eq!(spec.section, Section::Read);
    assert!(spec.positionals.is_empty(), "{:?}", spec.positionals);
}

/// The three records carry the authored example counts, pinned here so an
/// empty or shrunken list fails the tranche tests, not only the metadata
/// check.
#[test]
fn the_hygiene_records_carry_their_examples() {
    for (name, count) in [
        ("hygiene audit", 3),
        ("hygiene clean", 3),
        ("hygiene log", 2),
    ] {
        assert_eq!(record(name).examples.len(), count, "{name} examples");
    }
}

/// The tranche's defaults record what the consumers apply when the flag is
/// omitted: `cli_write_hygiene` keeps, and `clio_hygiene` scores at 0.40.
#[test]
fn the_hygiene_defaults_record_the_values_the_consumers_apply() {
    let find = |name: &str, flag: &str| {
        record(name)
            .documented
            .iter()
            .find(|candidate| candidate.long == flag)
            .unwrap_or_else(|| panic!("`{name}` documents `--{flag}`"))
    };
    assert_eq!(find("hygiene clean", "action").default, Some("keep"));
    let min_score = find("hygiene audit", "min-score");
    assert_eq!(min_score.default, Some("0.40"), "the recorded default");
    assert_eq!(
        min_score.default.expect("a default").parse::<f64>(),
        Ok(clio_hygiene::DEFAULT_MIN_SCORE),
        "min-score ties to the constant"
    );
}

/// The hygiene sibling file holds only the hygiene records: every record
/// name its source declares carries the `hygiene ` prefix and equals the
/// sibling's own `HYGIENE_RECORDS` list, its source declares exactly one
/// `CommandSpec` const per listed record, the registry's hygiene names are
/// exactly that list, and each listed record renders a hygiene usage line.
#[test]
fn the_hygiene_sibling_holds_only_the_hygiene_records() {
    let mut exported: Vec<&str> = super::HYGIENE_RECORDS
        .iter()
        .map(|spec| spec.name)
        .collect();
    assert!(!exported.is_empty(), "the sibling lists its records");
    exported.sort_unstable();
    for spec in super::HYGIENE_RECORDS {
        assert!(
            spec.name.starts_with("hygiene "),
            "{} outside the hygiene prefix",
            spec.name
        );
        assert!(
            spec.full_usage().starts_with("usage: clio hygiene "),
            "{} usage",
            spec.name
        );
    }
    let mut registered: Vec<&str> = crate::cli_command_registry::names()
        .filter(|name| name.starts_with("hygiene "))
        .collect();
    registered.sort_unstable();
    assert_eq!(registered, exported, "the registry's hygiene names");

    let source = include_str!("cli_command_registry_shared_workspace_hygiene.rs");
    let mut declared: Vec<&str> = Vec::new();
    let mut rest = source;
    while let Some(at) = rest.find("name: \"") {
        rest = &rest[at + "name: \"".len()..];
        let end = rest.find('"').expect("a closed record name");
        declared.push(&rest[..end]);
        rest = &rest[end..];
    }
    assert!(!declared.is_empty(), "the sibling declares its records");
    declared.sort_unstable();
    assert_eq!(
        declared, exported,
        "the sibling file's declared record names"
    );

    let record_consts = source.match_indices(": CommandSpec").count();
    assert_eq!(
        record_consts,
        exported.len(),
        "the sibling file's CommandSpec consts"
    );
}
