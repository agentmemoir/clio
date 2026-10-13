// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the triple records against today's
//! usage table and parser flag sets, and for the tranche-level checks over all
//! eight `GRAPH_RECORDS`. The belief, graph, and associations per-record tests
//! live in the `belief_graph` sibling's test module.
//!
//! ## Owns
//! - Byte equality of each triple record's `full_usage` and `usage_line` with
//!   the registered usage text
//! - The exact `parser_spec` value and boolean flag sequences per triple record
//! - Each triple record's summary, tool, section, positional shape, required
//!   bits, and authored example count
//! - The tranche list order, the `validate` contract, and the records' alias,
//!   alternate, note, and override surfaces
//!
//! ## Does not own
//! - The belief, graph, and associations usage and parser pins (see the
//!   `belief_graph` sibling's `tests` module)
//! - The registry lookups and validation (`cli_command_registry_tests`)
//! - The renderer's hardest shapes over pure records (`cli_command_spec_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the triple and tranche-level registry data.

use crate::cli_command_registry_test_support::{positionals, record};
use crate::cli_command_spec::{CommandSpec, Section};

/// The tranche's record names, in head-group order.
const NAMES: [&str; 8] = [
    "triple query",
    "triple add",
    "triple end",
    "belief history",
    "belief observe",
    "graph query",
    "graph link",
    "associations",
];

/// `triple query` renders today's usage text and builds today's parser sets;
/// its nested optional positionals make it the one record with an override.
#[test]
fn triple_query_matches_the_registered_usage_and_parser() {
    let spec = record("triple query");
    assert_eq!(
        spec.full_usage(),
        "usage: clio triple query [SUBJECT [PREDICATE [OBJECT]]] [--subject S] [--predicate P] [--object O] [--as-of TS] [--time-axis valid|transaction] [--explain] [--bank BANK] [--output json|text]",
        "triple query full usage"
    );
    assert_eq!(
        spec.usage_line(),
        "clio triple query [SUBJECT [PREDICATE [OBJECT]]] [--subject S] [--predicate P] [--object O] [--as-of TS] [--time-axis valid|transaction] [--explain] [--bank BANK] [--output json|text]",
        "triple query usage line"
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
            "predicate",
            "object",
            "as-of",
            "time-axis"
        ]
    );
    assert_eq!(
        spec.parser_spec().bool_flags(),
        ["no-input", "yes", "explain"]
    );
    assert_eq!(spec.summary, "match subject/predicate/object patterns");
    assert_eq!(spec.tool, Some("triple_query"));
    assert_eq!(spec.section, Section::Read);
    assert_eq!(
        positionals(spec),
        [("SUBJECT", false), ("PREDICATE", false), ("OBJECT", false)],
        "triple query positionals"
    );
    assert!(
        spec.required_flags().is_empty(),
        "triple query required flags"
    );
}

/// `triple add` renders today's usage text and builds today's parser sets; all
/// three of its positionals are required and none of its flags are.
#[test]
fn triple_add_matches_the_registered_usage_and_parser() {
    let spec = record("triple add");
    assert_eq!(
        spec.full_usage(),
        "usage: clio triple add S P O [--epistemic-kind fact|belief] [--source-type T] [--confidence N] [--context TEXT] [--valid-from TS] [--valid-until TS] [--supersede true|false] [--subject-id ID] [--update-rule discrete] [--dry-run] [--bank BANK] [--output json|text]\n       --epistemic-kind and --source-type are a pair: --source-type (user_stated|agent_inferred|third_party) is required with belief and must be omitted with fact",
        "triple add full usage"
    );
    assert_eq!(
        spec.usage_line(),
        "clio triple add S P O [--epistemic-kind fact|belief] [--source-type T] [--confidence N] [--context TEXT] [--valid-from TS] [--valid-until TS] [--supersede true|false] [--subject-id ID] [--update-rule discrete] [--dry-run] [--bank BANK] [--output json|text]",
        "triple add usage line"
    );
    assert_eq!(
        spec.parser_spec().value_flags(),
        [
            "db",
            "backend",
            "bank",
            "actor",
            "output",
            "epistemic-kind",
            "source-type",
            "confidence",
            "context",
            "valid-from",
            "valid-until",
            "supersede",
            "subject-id",
            "update-rule"
        ]
    );
    assert_eq!(
        spec.parser_spec().bool_flags(),
        ["no-input", "yes", "dry-run"]
    );
    assert_eq!(spec.summary, "write one subject/predicate/object fact");
    assert_eq!(spec.tool, Some("triple_add"));
    assert_eq!(spec.section, Section::Write);
    assert_eq!(
        positionals(spec),
        [("S", true), ("P", true), ("O", true)],
        "triple add positionals"
    );
    assert!(
        spec.required_flags().is_empty(),
        "triple add required flags"
    );
}

/// `triple end` renders today's usage text and builds today's parser sets,
/// including the required bit its refusal message is built from.
#[test]
fn triple_end_matches_the_registered_usage_and_parser() {
    let spec = record("triple end");
    assert_eq!(
        spec.full_usage(),
        "usage: clio triple end S P [--object O] --valid-until TS [--update-rule discrete] [--dry-run] [--bank BANK] [--output json|text]",
        "triple end full usage"
    );
    assert_eq!(
        spec.usage_line(),
        "clio triple end S P [--object O] --valid-until TS [--update-rule discrete] [--dry-run] [--bank BANK] [--output json|text]",
        "triple end usage line"
    );
    assert_eq!(
        spec.parser_spec().value_flags(),
        [
            "db",
            "backend",
            "bank",
            "actor",
            "output",
            "object",
            "valid-until",
            "update-rule"
        ]
    );
    assert_eq!(
        spec.parser_spec().bool_flags(),
        ["no-input", "yes", "dry-run"]
    );
    assert_eq!(spec.summary, "close an open fact without a replacement");
    assert_eq!(spec.tool, Some("triple_end"));
    assert_eq!(spec.section, Section::Write);
    assert_eq!(
        positionals(spec),
        [("S", true), ("P", true)],
        "triple end positionals"
    );
    assert_eq!(spec.required_flags(), ["valid-until"]);
}

/// The tranche lists these eight records once each, in head-group order.
#[test]
fn the_graph_records_tranche_lists_its_eight_records() {
    let listed: Vec<&str> = super::GRAPH_RECORDS.iter().map(|spec| spec.name).collect();
    assert_eq!(listed, NAMES);
}

/// The tranche's records pass `validate` against the catalog inventory.
#[test]
fn the_graph_records_tranche_validates_cleanly() {
    let inventory: Vec<&str> = crate::command_catalog::catalog_commands().collect();
    let records: Vec<&CommandSpec> = super::GRAPH_RECORDS.iter().collect();
    assert_eq!(
        crate::cli_command_registry::validate(&records, &inventory),
        Ok(())
    );
}

/// The write subcommands' required bits reach the registry-first accessor
/// unchanged, so no refusal message they build can drift from the record.
#[test]
fn the_write_subcommands_carry_their_required_bits() {
    assert_eq!(
        crate::cli_read_args::required_flags("triple end"),
        ["valid-until"]
    );
    assert_eq!(
        crate::cli_read_args::required_flags("belief observe"),
        ["confidence", "evidence", "source"]
    );
    assert_eq!(
        crate::cli_read_args::required_flags("graph link"),
        ["relationship"]
    );
}

/// These records carry no aliases, flag aliases, or alternates; only
/// `triple add` carries a note and only `triple query` carries a usage
/// override. This pins that none of them grew quietly.
#[test]
fn the_eight_records_carry_no_alias_or_alternate_surfaces() {
    for name in NAMES {
        let spec = record(name);
        assert!(spec.aliases.is_empty(), "{name} aliases");
        assert!(spec.flag_aliases.is_empty(), "{name} flag aliases");
        assert!(spec.usage_alternates.is_empty(), "{name} alternates");
        if name == "triple add" {
            assert_eq!(
                spec.notes,
                [super::SOURCE_TYPE_PAIRING_NOTE],
                "{name} notes"
            );
        } else {
            assert!(spec.notes.is_empty(), "{name} notes");
        }
        assert_eq!(
            spec.usage_override.is_some(),
            name == "triple query",
            "{name} override"
        );
    }
}

/// The triple records carry the authored example counts, pinned here so an
/// empty or shrunken list fails the tranche tests, not only the metadata
/// check. The belief, graph, and associations counts are pinned by the
/// sibling's test module.
#[test]
fn the_triple_records_carry_their_examples() {
    for (name, count) in [("triple query", 3), ("triple add", 3), ("triple end", 3)] {
        assert_eq!(record(name).examples.len(), count, "{name} examples");
    }
}
