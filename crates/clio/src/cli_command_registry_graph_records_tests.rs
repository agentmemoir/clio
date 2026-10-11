// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the triple, belief, graph, and
//! associations records against today's usage table and parser flag sets.
//!
//! ## Owns
//! - Byte equality of each record's `full_usage` and `usage_line` with the
//!   registered usage text
//! - The exact `parser_spec` value and boolean flag sequences per command
//! - Each record's summary, tool, section, positional values, and required bits
//!
//! ## Does not own
//! - The registry lookups and validation (`cli_command_registry_tests`)
//! - The renderer's hardest shapes over pure records (`cli_command_spec_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on these eight records' data.

use crate::cli_command_registry_test_support::record;
use crate::cli_command_spec::{CommandSpec, Positional, Section};

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

/// A required positional with this label, as these records carry them.
const fn required(label: &'static str) -> Positional {
    Positional {
        label,
        required: true,
        description: "",
    }
}

/// An optional positional with this label, as `triple query` carries them.
const fn optional(label: &'static str) -> Positional {
    Positional {
        label,
        required: false,
        description: "",
    }
}

/// `triple query` renders today's usage text and builds today's parser sets;
/// its nested optional positionals make it the one record with an override.
#[test]
fn triple_query_matches_the_registered_usage_and_parser() {
    let spec = record("triple query");
    assert_eq!(
        spec.full_usage(),
        "usage: clio triple query [SUBJECT [PREDICATE [OBJECT]]] [--subject S] [--predicate P] [--object O] [--as-of TS] [--time-axis valid|transaction] [--explain] [--output json|text]",
        "triple query full usage"
    );
    assert_eq!(
        spec.usage_line(),
        "clio triple query [SUBJECT [PREDICATE [OBJECT]]] [--subject S] [--predicate P] [--object O] [--as-of TS] [--time-axis valid|transaction] [--explain] [--output json|text]",
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
        spec.positionals,
        [
            optional("SUBJECT"),
            optional("PREDICATE"),
            optional("OBJECT")
        ]
        .as_slice()
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
        "usage: clio triple add S P O [--epistemic-kind fact|belief] [--source-type T] [--confidence N] [--context TEXT] [--valid-from TS] [--valid-until TS] [--supersede true|false] [--subject-id ID] [--update-rule discrete] [--dry-run] [--output json|text]\n       --epistemic-kind and --source-type are a pair: --source-type (user_stated|agent_inferred|third_party) is required with belief and must be omitted with fact",
        "triple add full usage"
    );
    assert_eq!(
        spec.usage_line(),
        "clio triple add S P O [--epistemic-kind fact|belief] [--source-type T] [--confidence N] [--context TEXT] [--valid-from TS] [--valid-until TS] [--supersede true|false] [--subject-id ID] [--update-rule discrete] [--dry-run] [--output json|text]",
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
        spec.positionals,
        [required("S"), required("P"), required("O")].as_slice()
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
        "usage: clio triple end S P [--object O] --valid-until TS [--update-rule discrete] [--dry-run] [--output json|text]",
        "triple end full usage"
    );
    assert_eq!(
        spec.usage_line(),
        "clio triple end S P [--object O] --valid-until TS [--update-rule discrete] [--dry-run] [--output json|text]",
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
    assert_eq!(spec.positionals, [required("S"), required("P")].as_slice());
    assert_eq!(spec.required_flags(), ["valid-until"]);
}

/// `belief history` renders today's usage text and builds today's parser sets.
#[test]
fn belief_history_matches_the_registered_usage_and_parser() {
    let spec = record("belief history");
    assert_eq!(
        spec.full_usage(),
        "usage: clio belief history BELIEF_ID_OR_PROPOSITION [--explain] [--output json|text]",
        "belief history full usage"
    );
    assert_eq!(
        spec.usage_line(),
        "clio belief history BELIEF_ID_OR_PROPOSITION [--explain] [--output json|text]",
        "belief history usage line"
    );
    assert_eq!(
        spec.parser_spec().value_flags(),
        ["db", "backend", "bank", "actor", "output"]
    );
    assert_eq!(
        spec.parser_spec().bool_flags(),
        ["no-input", "yes", "explain"]
    );
    assert_eq!(spec.summary, "one belief's confidence history");
    assert_eq!(spec.tool, Some("belief_history"));
    assert_eq!(spec.section, Section::Read);
    assert_eq!(
        spec.positionals,
        [required("BELIEF_ID_OR_PROPOSITION")].as_slice()
    );
    assert!(
        spec.required_flags().is_empty(),
        "belief history required flags"
    );
}

/// `belief observe` renders today's usage text and builds today's parser sets,
/// including the three required bits its refusal message is built from.
#[test]
fn belief_observe_matches_the_registered_usage_and_parser() {
    let spec = record("belief observe");
    assert_eq!(
        spec.full_usage(),
        "usage: clio belief observe PROP --confidence C --evidence E --source S [--as-of TS] [--subject-id ID] [--context TEXT] [--dry-run] [--output json|text]",
        "belief observe full usage"
    );
    assert_eq!(
        spec.usage_line(),
        "clio belief observe PROP --confidence C --evidence E --source S [--as-of TS] [--subject-id ID] [--context TEXT] [--dry-run] [--output json|text]",
        "belief observe usage line"
    );
    assert_eq!(
        spec.parser_spec().value_flags(),
        [
            "db",
            "backend",
            "bank",
            "actor",
            "output",
            "confidence",
            "evidence",
            "source",
            "as-of",
            "subject-id",
            "context"
        ]
    );
    assert_eq!(
        spec.parser_spec().bool_flags(),
        ["no-input", "yes", "dry-run"]
    );
    assert_eq!(spec.summary, "record evidence and a new confidence");
    assert_eq!(spec.tool, Some("belief_observe"));
    assert_eq!(spec.section, Section::Write);
    assert_eq!(spec.positionals, [required("PROP")].as_slice());
    assert_eq!(spec.required_flags(), ["confidence", "evidence", "source"]);
}

/// `graph query` renders today's usage text and builds today's parser sets.
#[test]
fn graph_query_matches_the_registered_usage_and_parser() {
    let spec = record("graph query");
    assert_eq!(
        spec.full_usage(),
        "usage: clio graph query SEED_ID [--max-hops N] [--edge-type coactivation|explicit] [--min-weight W] [--explain] [--output json|text]",
        "graph query full usage"
    );
    assert_eq!(
        spec.usage_line(),
        "clio graph query SEED_ID [--max-hops N] [--edge-type coactivation|explicit] [--min-weight W] [--explain] [--output json|text]",
        "graph query usage line"
    );
    assert_eq!(
        spec.parser_spec().value_flags(),
        [
            "db",
            "backend",
            "bank",
            "actor",
            "output",
            "max-hops",
            "edge-type",
            "min-weight"
        ]
    );
    assert_eq!(
        spec.parser_spec().bool_flags(),
        ["no-input", "yes", "explain"]
    );
    assert_eq!(spec.summary, "neighbours of one item, by edge type");
    assert_eq!(spec.tool, Some("graph_query"));
    assert_eq!(spec.section, Section::Read);
    assert_eq!(spec.positionals, [required("SEED_ID")].as_slice());
    assert!(
        spec.required_flags().is_empty(),
        "graph query required flags"
    );
}

/// `graph link` renders today's usage text and builds today's parser sets,
/// including the required bit its refusal message is built from.
#[test]
fn graph_link_matches_the_registered_usage_and_parser() {
    let spec = record("graph link");
    assert_eq!(
        spec.full_usage(),
        "usage: clio graph link A B --relationship R [--weight W] [--dry-run] [--output json|text]",
        "graph link full usage"
    );
    assert_eq!(
        spec.usage_line(),
        "clio graph link A B --relationship R [--weight W] [--dry-run] [--output json|text]",
        "graph link usage line"
    );
    assert_eq!(
        spec.parser_spec().value_flags(),
        [
            "db",
            "backend",
            "bank",
            "actor",
            "output",
            "relationship",
            "weight"
        ]
    );
    assert_eq!(
        spec.parser_spec().bool_flags(),
        ["no-input", "yes", "dry-run"]
    );
    assert_eq!(spec.summary, "add an explicit edge between two items");
    assert_eq!(spec.tool, Some("graph_link"));
    assert_eq!(spec.section, Section::Write);
    assert_eq!(spec.positionals, [required("A"), required("B")].as_slice());
    assert_eq!(spec.required_flags(), ["relationship"]);
}

/// `associations` renders today's usage text and builds today's parser sets.
#[test]
fn associations_matches_the_registered_usage_and_parser() {
    let spec = record("associations");
    assert_eq!(
        spec.full_usage(),
        "usage: clio associations ITEM_ID [--min-weight W] [--explain] [--output json|text]",
        "associations full usage"
    );
    assert_eq!(
        spec.usage_line(),
        "clio associations ITEM_ID [--min-weight W] [--explain] [--output json|text]",
        "associations usage line"
    );
    assert_eq!(
        spec.parser_spec().value_flags(),
        ["db", "backend", "bank", "actor", "output", "min-weight"]
    );
    assert_eq!(
        spec.parser_spec().bool_flags(),
        ["no-input", "yes", "explain"]
    );
    assert_eq!(spec.summary, "co-activation links for one item");
    assert_eq!(spec.tool, Some("associations"));
    assert_eq!(spec.section, Section::Read);
    assert_eq!(spec.positionals, [required("ITEM_ID")].as_slice());
    assert!(
        spec.required_flags().is_empty(),
        "associations required flags"
    );
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

/// These records carry no aliases, flag aliases, alternates, prose, or
/// examples; only `triple query` carries a usage override. This pins that none
/// of them grew quietly.
#[test]
fn the_eight_records_carry_no_extra_surfaces() {
    for name in NAMES {
        let spec = record(name);
        assert!(spec.aliases.is_empty(), "{name} aliases");
        assert!(spec.flag_aliases.is_empty(), "{name} flag aliases");
        assert!(spec.usage_alternates.is_empty(), "{name} alternates");
        assert!(spec.examples.is_empty(), "{name} examples");
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
