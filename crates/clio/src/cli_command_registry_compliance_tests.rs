// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the one `compliance` record against
//! the usage bytes it registers, the parser flag set it builds, and the
//! catalog inventory it validates against.
//!
//! ## Owns
//! - Byte equality of the record's `usage_line` and `full_usage` with the
//!   frozen registered usage text, override included
//! - The exact parser value/boolean flag sequences, the one repeatable flag,
//!   and the record's summary, tool, section, and positional values
//! - The group derivation: the registered `compliance` leaves and the surface
//!   subcommand list the dispatcher reads
//!
//! ## Does not own
//! - The registry lookups and validation (`cli_command_registry_tests`)
//! - The command implementation under test (`compliance_cli`)
//! - The frozen golden capture (`golden_repeatability_test`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the one compliance record's data.

use crate::cli_command_registry_test_support::{
    assert_example_count, assert_examples_name_their_record, record,
};
use crate::cli_command_spec::Section;

/// The frozen usage line: the surface documents `--output` on the line and
/// `--copy` in the options block, so the line is the record's own override and
/// a reworded or reassembled line fails here.
const USAGE: &str = "clio compliance inventory [--output json|text]";

/// `compliance inventory` renders the frozen usage text and builds the
/// record-derived parser sets, with `--copy` as the one repeatable flag.
#[test]
fn compliance_inventory_matches_the_registered_usage_and_parser() {
    let spec = record("compliance inventory");
    assert_eq!(spec.usage_line(), USAGE, "compliance inventory usage line");
    assert_eq!(spec.usage_override, Some(USAGE), "the line is pinned whole");
    assert_eq!(
        spec.full_usage(),
        format!("usage: {USAGE}\n       {note}", note = spec.notes[0]),
        "compliance inventory full usage"
    );
    assert_eq!(
        spec.parser_spec().value_flags(),
        ["db", "backend", "bank", "actor", "output", "copy"]
    );
    assert_eq!(spec.parser_spec().bool_flags(), ["no-input", "yes"]);
    assert_eq!(spec.tool, None);
    assert_eq!(spec.section, Section::Read);
    assert!(spec.positionals.is_empty(), "{:?}", spec.positionals);
}

/// The usage line keeps the canonical output spelling: `--output json|text`
/// with no bare `-o` spelling, and it does not name `--copy`, whose home is
/// the options block.
#[test]
fn the_usage_line_keeps_the_canonical_output_spelling() {
    let usage = record("compliance inventory").usage_line();
    assert!(usage.contains("[--output json|text]"), "{usage}");
    assert!(!usage.contains("-o json|text"), "{usage}");
    assert!(!usage.contains("--copy"), "{usage}");
}

/// `--copy` is the record's one repeatable value flag, so a caller can declare
/// several caller-held copies in one invocation.
#[test]
fn the_copy_flag_is_the_records_one_repeatable_flag() {
    let spec = record("compliance inventory");
    let repetable: Vec<&str> = spec
        .documented
        .iter()
        .filter(|flag| flag.repeatable)
        .map(|flag| flag.long)
        .collect();
    assert_eq!(repetable, ["copy"], "the record's repeatable flags");
    let copy = spec
        .documented
        .iter()
        .find(|flag| flag.long == "copy")
        .expect("the record documents --copy");
    assert_eq!(
        copy.value.expect("--copy takes a value").placeholder,
        "PATH"
    );
}

/// The record binds no MCP tool, declares no aliases or alternates, and
/// carries two to four examples that name their own command.
#[test]
fn the_compliance_record_carries_no_extra_surfaces() {
    let spec = record("compliance inventory");
    assert!(spec.aliases.is_empty(), "compliance inventory aliases");
    assert!(spec.flag_aliases.is_empty(), "flag aliases");
    assert!(spec.usage_alternates.is_empty(), "alternates");
    assert_eq!(spec.notes.len(), 1, "compliance inventory notes");
    assert_example_count("compliance inventory", spec);
    assert_examples_name_their_record("compliance inventory", spec);
}

/// `compliance review-export` renders its own usage line and builds its parser
/// sets: `--out` is required, and it binds no MCP tool.
#[test]
fn compliance_review_export_matches_its_registered_usage_and_parser() {
    let spec = record("compliance review-export");
    assert_eq!(
        spec.full_usage(),
        "usage: clio compliance review-export --out FILE [--limit N] [--cursor C] [--bank BANK] [--actor ACTOR] [--output json|text]\n       writes the ordered local review events as content-free rows (sequence position, action id, state) under durable destination custody: the destination is recorded pending before any bytes land, an existing file is never overwritten, and only a harness or system actor may export",
        "compliance review-export full usage"
    );
    assert_eq!(
        spec.parser_spec().value_flags(),
        [
            "db", "backend", "bank", "actor", "output", "out", "limit", "cursor"
        ]
    );
    assert_eq!(spec.parser_spec().bool_flags(), ["no-input", "yes"]);
    assert_eq!(spec.required_flags(), ["out"]);
    assert_eq!(spec.tool, None);
    assert_eq!(spec.section, Section::Write);
    assert!(spec.positionals.is_empty(), "{:?}", spec.positionals);
    assert_eq!(spec.notes.len(), 1, "compliance review-export notes");
    assert_example_count("compliance review-export", spec);
    assert_examples_name_their_record("compliance review-export", spec);
}

/// The registered `compliance` leaves are exactly the tranche, once each, and
/// the surface's subcommand list is the derivation the dispatcher reads.
#[test]
fn the_compliance_leaves_register_once_in_capture_order() {
    let registered: Vec<&str> = crate::cli_command_registry::all()
        .filter(|spec| spec.name.starts_with("compliance "))
        .map(|spec| spec.name)
        .collect();
    assert_eq!(
        registered,
        [
            "compliance inventory",
            "compliance review-export",
            "compliance request-start",
            "compliance request-status",
            "compliance request-retry",
            "compliance request-copy",
            "compliance request-exception",
            "compliance request-events",
            "compliance request-close",
            "compliance peer-status",
            "compliance peer-retry",
            "compliance peer-disposition",
        ],
        "the registered leaves"
    );
    let tranche: Vec<&str> = crate::cli_command_registry_compliance::COMPLIANCE_LEAVES
        .iter()
        .map(|spec| spec.name)
        .collect();
    assert_eq!(tranche, registered, "the tranche and the registry agree");
    assert_eq!(
        crate::compliance_cli::compliance_subcommands(),
        [
            "inventory",
            "review-export",
            "request-start",
            "request-status",
            "request-retry",
            "request-copy",
            "request-exception",
            "request-events",
            "request-close",
            "peer-status",
            "peer-retry",
            "peer-disposition",
        ],
        "the derived subcommand list"
    );
}

/// The real registry satisfies `validate` against the catalog inventory with
/// the compliance record present.
#[test]
fn validate_accepts_the_compliance_record_against_the_catalog_inventory() {
    let specs: Vec<&crate::cli_command_spec::CommandSpec> =
        crate::cli_command_registry::all().collect();
    let inventory: Vec<&str> = crate::command_catalog::catalog_commands().collect();
    assert_eq!(
        crate::cli_command_registry::validate(&specs, &inventory),
        Ok(())
    );
}

/// The record's flags carry complete metadata, and the two flag-spelling
/// checks run over it.
#[test]
fn metadata_check_accepts_the_compliance_record() {
    let specs: Vec<&crate::cli_command_spec::CommandSpec> =
        crate::cli_command_registry_compliance::COMPLIANCE_LEAVES
            .iter()
            .collect();
    assert_eq!(crate::cli_command_registry::check_metadata(&specs), Ok(()));
}
