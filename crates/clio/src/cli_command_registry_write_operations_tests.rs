// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the operation-write tranche: each record's bytes against
//! the registered usage text, and its exact parser-visible flag sets, the
//! `batch` stdin alternate and the pinned `erase` line included.
//!
//! ## Owns
//! - Byte equality of each record's `full_usage` and `usage_line` with the
//!   usage text the write lookup resolves
//! - The exact `parser_spec` value and boolean flag sequences per command
//! - Each record's summary, tool, section, positional, and required-flag values
//! - The claims `export`'s three narrowing flags' help makes about the graph
//!   and about the collections the narrowing leaves whole
//! - The tranche's `validate` contract against the catalog inventory
//! - The tranche's flag metadata: the enum lists the consumers accept and the
//!   defaults the tools apply
//!
//! ## Does not own
//! - The registry lookups and `validate` itself (`cli_command_registry_tests`)
//! - The renderer's shapes over pure records (`cli_command_spec_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the operation-write records' data.

use crate::cli_command_registry_test_support::{positionals, record};
use crate::cli_command_spec::{CommandSpec, Section, ValueKind};

/// One record's documented flag, as the metadata tests read it.
fn documented_flag(name: &str, flag: &str) -> &'static crate::cli_command_spec::Flag {
    record(name)
        .documented
        .iter()
        .find(|candidate| candidate.long == flag)
        .unwrap_or_else(|| panic!("`{name}` documents `--{flag}`"))
}

/// The tranche's records, as the validation contract consumes them.
fn tranche() -> Vec<&'static CommandSpec> {
    super::WRITE_OPERATIONS.iter().collect()
}

/// The tranche's records pass `validate` against the catalog inventory.
#[test]
fn the_write_operations_tranche_validates_cleanly() {
    let inventory: Vec<&str> = crate::command_catalog::catalog_commands().collect();
    assert_eq!(
        crate::cli_command_registry::validate(&tranche(), &inventory),
        Ok(())
    );
}

/// The tranche lists its five records once each, in the declared order.
#[test]
fn the_tranche_lists_its_five_records_in_declared_order() {
    let names: Vec<&str> = super::WRITE_OPERATIONS
        .iter()
        .map(|spec| spec.name)
        .collect();
    assert_eq!(names, ["validate", "batch", "export", "import", "erase"]);
}

/// Every record answers the write lookup with its own bytes, so the authored
/// rows can be deleted without changing either surface.
///
/// The independent input here is the lookup's dispatch: the record's bytes are
/// pinned literally by the per-record tests and the frozen fixture, so this
/// assertion fails when `usage_for` stops reading records and returns a stale
/// table row's bytes or nothing.
#[test]
fn every_record_answers_the_lookup_with_its_own_bytes() {
    for spec in &tranche() {
        assert_eq!(
            crate::cli_help_usage::usage_for(spec.name),
            Some(spec.full_usage()),
            "{} resolves through the write lookup",
            spec.name
        );
        assert_eq!(
            crate::cli_help_usage::usage_line(spec.name),
            Some(spec.usage_line()),
            "{} usage line resolves through the lookup",
            spec.name
        );
        assert!(!spec.summary.is_empty(), "{} summary", spec.name);
    }
}

/// `validate` renders today's usage text and builds today's parser sets.
#[test]
fn validate_matches_the_registered_usage_and_parser() {
    let spec = record("validate");
    assert_eq!(
        spec.full_usage(),
        "usage: clio validate ITEM_ID --action attest|update|invalidate|delete [--note N] [--new-content C] [--bank BANK] [--actor ACTOR] [--output json|text]",
        "validate full usage"
    );
    assert_eq!(
        spec.parser_spec().value_flags(),
        [
            "db",
            "backend",
            "bank",
            "actor",
            "output",
            "action",
            "note",
            "new-content"
        ]
    );
    assert_eq!(spec.parser_spec().bool_flags(), ["no-input", "yes"]);
    assert_eq!(spec.required_flags(), ["action"]);
    assert_eq!(
        spec.summary,
        "attest, update, invalidate, or delete one item"
    );
    assert_eq!(spec.tool, Some("validate"));
    assert_eq!(spec.section, Section::Write);
    assert_eq!(positionals(spec), [("ITEM_ID", true)]);
}

/// `batch` renders the stdin alternate and builds today's parser sets.
#[test]
fn batch_renders_the_stdin_alternate_and_the_parser_sets() {
    let spec = record("batch");
    assert_eq!(
        spec.full_usage(),
        "usage: clio batch --file ops.json [--dry-run] [--bank BANK] [--actor ACTOR] [--output json|text]\n       or: clio batch --file - (reads operations JSON from stdin)",
        "batch full usage"
    );
    assert_eq!(
        spec.usage_line(),
        "clio batch --file ops.json [--dry-run] [--bank BANK] [--actor ACTOR] [--output json|text]",
        "batch usage line"
    );
    assert_eq!(
        spec.usage_alternates,
        ["clio batch --file - (reads operations JSON from stdin)"]
    );
    assert_eq!(
        spec.parser_spec().value_flags(),
        ["db", "backend", "bank", "actor", "output", "file"]
    );
    assert_eq!(
        spec.parser_spec().bool_flags(),
        ["no-input", "yes", "dry-run"]
    );
    assert_eq!(spec.required_flags(), ["file"]);
    assert_eq!(
        spec.summary,
        "apply a list of operations in one transaction"
    );
    assert_eq!(spec.tool, Some("batch"));
    assert_eq!(spec.section, Section::Write);
    assert!(spec.positionals.is_empty(), "{:?}", spec.positionals);
}

/// `export` renders today's usage text and builds today's parser sets.
#[test]
fn export_matches_the_registered_usage_and_parser() {
    let spec = record("export");
    assert_eq!(
        spec.full_usage(),
        "usage: clio export --out FILE [--content-mode dsar_plaintext|ciphertext_backup] [--category C] [--time-start TS] [--time-end TS] [--include-archived true|false] [--include-discarded true|false] [--subject S] [--bank BANK] [--actor ACTOR] [--output json|text]",
        "export full usage"
    );
    assert_eq!(
        spec.parser_spec().value_flags(),
        [
            "db",
            "backend",
            "bank",
            "actor",
            "output",
            "out",
            "content-mode",
            "category",
            "time-start",
            "time-end",
            "include-archived",
            "include-discarded",
            "subject"
        ]
    );
    assert_eq!(spec.parser_spec().bool_flags(), ["no-input", "yes"]);
    assert_eq!(spec.required_flags(), ["out"]);
    assert_eq!(
        spec.summary,
        "(manifest-backed JSON bundle; requires --actor harness|system)"
    );
    assert_eq!(spec.tool, Some("export"));
    assert_eq!(spec.section, Section::Write);
    assert!(spec.positionals.is_empty(), "{:?}", spec.positionals);
}

/// The three `export` narrowing flags say what the narrowing does to the graph
/// and to the collections it cannot reach, and name the flag that scopes the
/// whole bundle.
#[test]
fn the_export_narrowing_flags_explain_the_graph_and_the_whole_collections() {
    for (flag, consumer) in [
        ("category", "SemanticCategory::WIRE"),
        ("time-start", "`time_start`"),
        ("time-end", "`time_end`"),
    ] {
        let description = documented_flag("export", flag).description;
        assert!(
            description.starts_with("Narrows the items collection"),
            "`--{flag}` names the collection it narrows: {description}"
        );
        assert!(
            description.contains(consumer),
            "`--{flag}` names its consumer: {description}"
        );
        assert!(
            description.contains("triples and association edges follow it")
                && description.contains("one whose endpoint is filtered out is dropped"),
            "`--{flag}` states the graph rule: {description}"
        );
        let whole = "the filters do not reach persona_stable, persona_preferences, tasks, \
                     failures, beliefs, and tombstones, which stay whole, and the bundle \
                     manifest records it";
        assert!(
            description.contains(whole),
            "`--{flag}` names the collections it leaves whole: {description}"
        );
        assert!(
            description.contains("`--subject` scopes the whole bundle"),
            "`--{flag}` defers the bundle scope to `--subject`: {description}"
        );
    }
}

/// `import` renders today's usage text and builds today's parser sets.
#[test]
fn import_matches_the_registered_usage_and_parser() {
    let spec = record("import");
    assert_eq!(
        spec.full_usage(),
        "usage: clio import --source FILE [--dry-run] [--force] [--reason R] [--bank BANK] [--output json|text]",
        "import full usage"
    );
    assert_eq!(
        spec.parser_spec().value_flags(),
        [
            "db", "backend", "bank", "actor", "output", "source", "reason"
        ]
    );
    assert_eq!(
        spec.parser_spec().bool_flags(),
        ["no-input", "yes", "dry-run", "force"]
    );
    assert_eq!(spec.required_flags(), ["source"]);
    assert_eq!(spec.summary, "load a bundle produced by `clio export`");
    assert_eq!(spec.tool, Some("import"));
    assert_eq!(spec.section, Section::Write);
    assert!(spec.positionals.is_empty(), "{:?}", spec.positionals);
}

/// `erase` pins its printed line, carries the hidden `--dry-run`, and prints
/// its actor choice instead of the global spelling.
#[test]
fn erase_pins_the_line_and_hides_the_dry_run_flag() {
    let spec = record("erase");
    assert_eq!(
        spec.full_usage(),
        "usage: clio erase --subject S --basis B --request R [--reason-code C] --confirm [--actor harness|system] [--bank BANK] [--output json|text]\n       compliance erasure is IRREVERSIBLE (DEK destruction) and is distinct from `discard` and hygiene cleanup",
        "erase full usage"
    );
    assert_eq!(
        spec.usage_line(),
        "clio erase --subject S --basis B --request R [--reason-code C] --confirm [--actor harness|system] [--bank BANK] [--output json|text]",
        "erase usage line"
    );
    assert!(
        !spec.usage_line().contains("--dry-run"),
        "the hidden --dry-run flag must not print in the line: {}",
        spec.usage_line()
    );
    assert_eq!(
        spec.parser_spec().value_flags(),
        [
            "db",
            "backend",
            "bank",
            "actor",
            "output",
            "subject",
            "basis",
            "request",
            "reason-code"
        ]
    );
    assert_eq!(
        spec.parser_spec().bool_flags(),
        ["no-input", "yes", "confirm", "dry-run"]
    );
    assert_eq!(
        spec.required_flags(),
        ["subject", "basis", "request"],
        "the confirm gate owns --confirm"
    );
    assert_eq!(
        spec.summary,
        "IRREVERSIBLE compliance erasure (DEK destruction); distinct from `discard` and hygiene cleanup"
    );
    assert_eq!(spec.tool, Some("erase_request"));
    assert_eq!(spec.section, Section::Write);
    assert!(spec.positionals.is_empty(), "{:?}", spec.positionals);
}

/// Every documented flag in the tranche carries complete metadata: a
/// description, and for a value flag a placeholder and a non-empty value kind.
#[test]
fn the_write_operations_tranche_has_complete_flag_metadata() {
    let specs: Vec<&CommandSpec> = super::WRITE_OPERATIONS.iter().collect();
    assert_eq!(crate::cli_command_registry::check_metadata(&specs), Ok(()));
}

/// Every documented flag carries the kind its consumer parses: the enum
/// lists name exactly the accepted values (`cli_write_shared`'s four review
/// actions, `cli_write_portability`'s two content modes and two include
/// labels and the `clio_types` category wire table, and the `erase_request`
/// tool's two actors), and every remaining flag stays free text or the shared `--output` global.
#[test]
fn the_write_operations_flag_kinds_match_their_consumers() {
    for spec in super::WRITE_OPERATIONS {
        for flag in spec.documented {
            let Some(value) = flag.value else { continue };
            let expected = match (spec.name, flag.long) {
                ("validate", "action") => {
                    ValueKind::Enum(&["attest", "update", "invalidate", "delete"])
                }
                ("export", "content-mode") => {
                    ValueKind::Enum(&["dsar_plaintext", "ciphertext_backup"])
                }
                ("export", "category") => ValueKind::Enum(clio_types::SemanticCategory::WIRE),
                ("export", "include-archived" | "include-discarded") => {
                    ValueKind::Enum(&["true", "false"])
                }
                ("erase", "actor") => ValueKind::Enum(&["harness", "system"]),
                (_, "output") => ValueKind::Enum(&["json", "text"]),
                _ => ValueKind::Text,
            };
            assert_eq!(value.kind, expected, "`{} --{}`", spec.name, flag.long);
        }
    }
}

/// The recorded defaults are the values the tools apply when the flag is
/// omitted: the export tool bundles plaintext with archived and discarded
/// rows, and compliance erasure defaults to the crypto-shred reason code,
/// whose named constant lives in `clio_compliance`, outside this crate's
/// dependency set.
#[test]
fn the_recorded_defaults_are_the_values_the_tools_apply() {
    for (name, flag, default) in [
        ("export", "content-mode", Some("dsar_plaintext")),
        ("export", "include-archived", Some("true")),
        ("export", "include-discarded", Some("true")),
        ("erase", "reason-code", Some("COMPLIANCE_CRYPTO_SHRED")),
    ] {
        assert_eq!(
            documented_flag(name, flag).default,
            default,
            "`{name} --{flag}` default"
        );
    }
}
