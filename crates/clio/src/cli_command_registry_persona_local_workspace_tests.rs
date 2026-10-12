// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the scratchpad and canonical
//! records against today's usage table and parser flag sets: byte equality of
//! each record's `full_usage` and `usage_line`, the exact `parser_spec` flag
//! sequences, and each record's summary, tool, section, positional shape, and
//! authored example count.
//!
//! ## Owns
//! - The five records' usage and parser pins
//! - Each record's summary, tool, section, positional shape, and example count
//!
//! ## Does not own
//! - The persona and maintenance records and the tranche-level checks
//!   (`cli_command_registry_persona_local_tests`)
//! - The flag metadata contract
//!   (`cli_command_registry_persona_local_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the scratchpad and canonical records.

use crate::cli_command_registry_test_support::{positionals, record};
use crate::cli_command_spec::Section;

/// `scratchpad write` renders today's usage text and builds today's parser sets.
#[test]
fn scratchpad_write_matches_the_registered_usage_and_parser() {
    let spec = record("scratchpad write");
    assert_eq!(
        spec.full_usage(),
        "usage: clio scratchpad write KEY CONTENT [--session S] [--bank BANK] [--actor ACTOR] [--output json|text]"
    );
    assert_eq!(
        spec.parser_spec().value_flags(),
        ["db", "backend", "bank", "actor", "output", "session"]
    );
    assert_eq!(spec.parser_spec().bool_flags(), ["no-input", "yes"]);
    assert_eq!(spec.summary, "store an ephemeral key for this session");
    assert_eq!(spec.tool, Some("scratchpad_write"));
    assert_eq!(spec.section, Section::Write);
    assert_eq!(
        positionals(spec),
        [("KEY", true), ("CONTENT", true)],
        "scratchpad write positionals"
    );
}

/// `scratchpad read` renders today's usage text and builds today's parser sets.
#[test]
fn scratchpad_read_matches_the_registered_usage_and_parser() {
    let spec = record("scratchpad read");
    assert_eq!(
        spec.full_usage(),
        "usage: clio scratchpad read [KEY] [--session S] [--bank BANK] [--actor ACTOR] [--output json|text]"
    );
    assert_eq!(
        spec.parser_spec().value_flags(),
        ["db", "backend", "bank", "actor", "output", "session"]
    );
    assert_eq!(spec.parser_spec().bool_flags(), ["no-input", "yes"]);
    assert_eq!(spec.summary, "read one key, or the whole pad");
    assert_eq!(spec.tool, Some("scratchpad_read"));
    assert_eq!(spec.section, Section::Read);
    assert_eq!(
        positionals(spec),
        [("KEY", false)],
        "scratchpad read positionals"
    );
}

/// `scratchpad clear` renders today's usage text and builds today's parser sets.
#[test]
fn scratchpad_clear_matches_the_registered_usage_and_parser() {
    let spec = record("scratchpad clear");
    assert_eq!(
        spec.full_usage(),
        "usage: clio scratchpad clear [KEY] [--session S] [--bank BANK] [--actor ACTOR] [--output json|text]"
    );
    assert_eq!(
        spec.parser_spec().value_flags(),
        ["db", "backend", "bank", "actor", "output", "session"]
    );
    assert_eq!(spec.parser_spec().bool_flags(), ["no-input", "yes"]);
    assert_eq!(spec.summary, "drop one key, or the whole pad");
    assert_eq!(spec.tool, Some("scratchpad_clear"));
    assert_eq!(spec.section, Section::Write);
    assert_eq!(
        positionals(spec),
        [("KEY", false)],
        "scratchpad clear positionals"
    );
}

/// `canonical put` renders today's usage text and carries the required bits
/// its refusal path already reads by verb name.
#[test]
fn canonical_put_matches_the_registered_usage_and_parser() {
    let spec = record("canonical put");
    assert_eq!(
        spec.full_usage(),
        "usage: clio canonical put KEY VALUE --category C --epistemic-kind K [--source-type T] [--confidence N] [--source-text TEXT] [--evidence-ref E] [--context C] [--dry-run] [--output json|text]\n       --epistemic-kind and --source-type are a pair: --source-type (user_stated|agent_inferred|third_party) is required with belief and must be omitted with fact"
    );
    assert_eq!(
        spec.required_flags(),
        ["category", "epistemic-kind"],
        "the refusal path reads these from the record now"
    );
    assert_eq!(
        spec.parser_spec().value_flags(),
        [
            "db",
            "backend",
            "bank",
            "actor",
            "output",
            "category",
            "epistemic-kind",
            "source-type",
            "confidence",
            "source-text",
            "evidence-ref",
            "context"
        ]
    );
    assert_eq!(
        spec.parser_spec().bool_flags(),
        ["no-input", "yes", "dry-run"]
    );
    assert_eq!(
        spec.summary,
        "gated single-slot upsert with bi-temporal supersession"
    );
    assert_eq!(spec.tool, Some("canonical_put"));
    assert_eq!(spec.section, Section::Write);
    assert_eq!(
        positionals(spec),
        [("KEY", true), ("VALUE", true)],
        "canonical put positionals"
    );
}

/// `canonical get` renders today's usage text and builds today's parser sets.
#[test]
fn canonical_get_matches_the_registered_usage_and_parser() {
    let spec = record("canonical get");
    assert_eq!(
        spec.full_usage(),
        "usage: clio canonical get KEY [--as-of TS] [--output json|text]"
    );
    assert_eq!(
        spec.parser_spec().value_flags(),
        ["db", "backend", "bank", "actor", "output", "as-of"]
    );
    assert_eq!(spec.parser_spec().bool_flags(), ["no-input", "yes"]);
    assert_eq!(spec.summary, "read one canonical key");
    assert_eq!(spec.tool, Some("canonical_get"));
    assert_eq!(spec.section, Section::Read);
    assert_eq!(
        positionals(spec),
        [("KEY", true)],
        "canonical get positionals"
    );
}

/// The five records carry the authored example counts, pinned here so an empty
/// or shrunken list fails the tranche tests, not only the metadata check.
#[test]
fn the_workspace_records_carry_their_examples() {
    for (name, count) in [
        ("scratchpad write", 3),
        ("scratchpad read", 3),
        ("scratchpad clear", 2),
        ("canonical put", 3),
        ("canonical get", 3),
    ] {
        assert_eq!(record(name).examples.len(), count, "{name} examples");
    }
}
