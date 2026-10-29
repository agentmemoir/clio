// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the belief, graph, and associations
//! records against today's usage table and parser flag sets: byte equality of
//! each record's `full_usage` and `usage_line`, the exact `parser_spec` flag
//! sequences, and each record's summary, tool, section, positional shape,
//! required bits, and authored example count.
//!
//! ## Owns
//! - The five records' usage and parser pins
//! - Each record's summary, tool, section, positional shape, and example count
//!
//! ## Does not own
//! - The triple records and the tranche list
//!   (`cli_command_registry_graph_records_tests`)
//! - The tranche's flag metadata contract
//!   (`cli_command_registry_graph_records_metadata_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the belief, graph, and associations records.

use crate::cli_command_registry_test_support::{positionals, record};
use crate::cli_command_spec::Section;

/// `belief history` renders today's usage text and builds today's parser sets.
#[test]
fn belief_history_matches_the_registered_usage_and_parser() {
    let spec = record("belief history");
    assert_eq!(
        spec.full_usage(),
        "usage: clio belief history BELIEF_ID_OR_PROPOSITION [--bank BANK] [--output json|text]",
        "belief history full usage"
    );
    assert_eq!(
        spec.usage_line(),
        "clio belief history BELIEF_ID_OR_PROPOSITION [--bank BANK] [--output json|text]",
        "belief history usage line"
    );
    assert_eq!(
        spec.parser_spec().value_flags(),
        ["db", "backend", "bank", "actor", "output"]
    );
    assert_eq!(spec.parser_spec().bool_flags(), ["no-input", "yes"]);
    assert_eq!(spec.summary, "one belief's confidence history");
    assert_eq!(spec.tool, Some("belief_history"));
    assert_eq!(spec.section, Section::Read);
    assert_eq!(
        positionals(spec),
        [("BELIEF_ID_OR_PROPOSITION", true)],
        "belief history positionals"
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
        "usage: clio belief observe PROP --confidence C --evidence E --source S [--as-of TS] [--subject-id ID] [--context TEXT] [--dry-run] [--bank BANK] [--output json|text]",
        "belief observe full usage"
    );
    assert_eq!(
        spec.usage_line(),
        "clio belief observe PROP --confidence C --evidence E --source S [--as-of TS] [--subject-id ID] [--context TEXT] [--dry-run] [--bank BANK] [--output json|text]",
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
    assert_eq!(
        positionals(spec),
        [("PROP", true)],
        "belief observe positionals"
    );
    assert_eq!(spec.required_flags(), ["confidence", "evidence", "source"]);
}

/// `graph query` renders today's usage text and builds today's parser sets.
#[test]
fn graph_query_matches_the_registered_usage_and_parser() {
    let spec = record("graph query");
    assert_eq!(
        spec.full_usage(),
        "usage: clio graph query SEED_ID [--max-hops N] [--edge-type coactivation|explicit] [--min-weight W] [--bank BANK] [--output json|text]",
        "graph query full usage"
    );
    assert_eq!(
        spec.usage_line(),
        "clio graph query SEED_ID [--max-hops N] [--edge-type coactivation|explicit] [--min-weight W] [--bank BANK] [--output json|text]",
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
    assert_eq!(spec.parser_spec().bool_flags(), ["no-input", "yes"]);
    assert_eq!(spec.summary, "neighbours of one item, by edge type");
    assert_eq!(spec.tool, Some("graph_query"));
    assert_eq!(spec.section, Section::Read);
    assert_eq!(
        positionals(spec),
        [("SEED_ID", true)],
        "graph query positionals"
    );
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
        "usage: clio graph link A B --relationship R [--weight W] [--dry-run] [--bank BANK] [--output json|text]",
        "graph link full usage"
    );
    assert_eq!(
        spec.usage_line(),
        "clio graph link A B --relationship R [--weight W] [--dry-run] [--bank BANK] [--output json|text]",
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
    assert_eq!(
        positionals(spec),
        [("A", true), ("B", true)],
        "graph link positionals"
    );
    assert_eq!(spec.required_flags(), ["relationship"]);
}

/// `associations` renders today's usage text and builds today's parser sets.
#[test]
fn associations_matches_the_registered_usage_and_parser() {
    let spec = record("associations");
    assert_eq!(
        spec.full_usage(),
        "usage: clio associations ITEM_ID [--min-weight W] [--bank BANK] [--output json|text]",
        "associations full usage"
    );
    assert_eq!(
        spec.usage_line(),
        "clio associations ITEM_ID [--min-weight W] [--bank BANK] [--output json|text]",
        "associations usage line"
    );
    assert_eq!(
        spec.parser_spec().value_flags(),
        ["db", "backend", "bank", "actor", "output", "min-weight"]
    );
    assert_eq!(spec.parser_spec().bool_flags(), ["no-input", "yes"]);
    assert_eq!(spec.summary, "co-activation links for one item");
    assert_eq!(spec.tool, Some("associations"));
    assert_eq!(spec.section, Section::Read);
    assert_eq!(
        positionals(spec),
        [("ITEM_ID", true)],
        "associations positionals"
    );
    assert!(
        spec.required_flags().is_empty(),
        "associations required flags"
    );
}

/// The five records carry the authored example counts, pinned here so an empty
/// or shrunken list fails the tranche tests, not only the metadata check.
#[test]
fn the_belief_graph_records_carry_their_examples() {
    for (name, count) in [
        ("belief history", 2),
        ("belief observe", 3),
        ("graph query", 3),
        ("graph link", 3),
        ("associations", 3),
    ] {
        assert_eq!(record(name).examples.len(), count, "{name} examples");
    }
}
