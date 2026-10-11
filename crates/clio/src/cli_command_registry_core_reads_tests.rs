// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the five core read records against
//! today's usage table and parser flag sets.
//!
//! ## Owns
//! - Byte equality of each record's `full_usage` and `usage_line` with the
//!   registered usage text
//! - The exact `parser_spec` value and boolean flag sequences per command
//! - Each record's summary, tool, section, and positional values
//!
//! ## Does not own
//! - The registry lookups and validation (`cli_command_registry_tests`)
//! - The renderer's hardest shapes over pure records (`cli_command_spec_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the five core read records' data.

use crate::cli_command_spec::{CommandSpec, Positional, Section};

/// One registered record, or a failure that names the missing command.
fn record(name: &str) -> &'static CommandSpec {
    crate::cli_command_registry::find(name).unwrap_or_else(|| panic!("`{name}` is registered"))
}

/// `recall` renders today's usage text and builds today's parser sets.
#[test]
fn recall_matches_the_registered_usage_and_parser() {
    let spec = record("recall");
    assert_eq!(
        spec.full_usage(),
        "usage: clio recall QUERY [--bank BANK] [--limit N] [--domains a,b] [--as-of TS] [--query-anchor TS] [--time-axis valid|transaction] [--min-semantic N] [--min-keyword N] [--min-reranker N] [--min-final N] [--no-entities] [--entity-summary] [--output json|text]\n       a score floor is a relative filter over this query's own scores, not a calibrated relevance threshold; --min-semantic and --min-keyword prune one retrieval arm each, and --no-entities cannot be combined with --entity-summary; --query-anchor must be RFC 3339 with an explicit UTC offset, and a relative phrase without an anchor contributes nothing (no server-time fallback)",
        "recall full usage"
    );
    assert_eq!(
        spec.usage_line(),
        "clio recall QUERY [--bank BANK] [--limit N] [--domains a,b] [--as-of TS] [--query-anchor TS] [--time-axis valid|transaction] [--min-semantic N] [--min-keyword N] [--min-reranker N] [--min-final N] [--no-entities] [--entity-summary] [--output json|text]",
        "recall usage line"
    );
    assert_eq!(
        spec.parser_spec().value_flags(),
        [
            "db",
            "backend",
            "bank",
            "actor",
            "output",
            "limit",
            "domains",
            "as-of",
            "query-anchor",
            "time-axis",
            "min-semantic",
            "min-keyword",
            "min-reranker",
            "min-final"
        ]
    );
    assert_eq!(
        spec.parser_spec().bool_flags(),
        ["no-input", "yes", "no-entities", "entity-summary"]
    );
    assert_eq!(
        spec.summary,
        "score floors are relative filters over this query's own scores, not calibrated thresholds"
    );
    assert_eq!(spec.tool, Some("retrieve"));
    assert_eq!(spec.section, Section::Read);
    assert_eq!(
        spec.positionals,
        [Positional {
            label: "QUERY",
            required: true,
            description: ""
        }]
        .as_slice()
    );
}

/// `get` renders today's usage text and builds today's parser sets.
#[test]
fn get_matches_the_registered_usage_and_parser() {
    let spec = record("get");
    assert_eq!(
        spec.full_usage(),
        "usage: clio get ID [--snapshot|--gist] [--output json|text]",
        "get full usage"
    );
    assert_eq!(
        spec.usage_line(),
        "clio get ID [--snapshot|--gist] [--output json|text]",
        "get usage line"
    );
    assert_eq!(
        spec.parser_spec().value_flags(),
        ["db", "backend", "bank", "actor", "output"]
    );
    assert_eq!(
        spec.parser_spec().bool_flags(),
        ["no-input", "yes", "snapshot", "gist"]
    );
    assert_eq!(spec.summary, "fetch one memory by id");
    assert_eq!(spec.tool, Some("get"));
    assert_eq!(spec.section, Section::Read);
    assert_eq!(
        spec.positionals,
        [Positional {
            label: "ID",
            required: true,
            description: ""
        }]
        .as_slice()
    );
}

/// `show` is its own visible record over the same tool, with the same usage
/// text and parser sets as `get`.
#[test]
fn show_matches_the_registered_usage_and_parser() {
    let spec = record("show");
    assert_eq!(
        spec.full_usage(),
        "usage: clio show ID [--snapshot|--gist] [--output json|text]",
        "show full usage"
    );
    assert_eq!(
        spec.usage_line(),
        "clio show ID [--snapshot|--gist] [--output json|text]",
        "show usage line"
    );
    assert_eq!(
        spec.parser_spec().value_flags(),
        ["db", "backend", "bank", "actor", "output"]
    );
    assert_eq!(
        spec.parser_spec().bool_flags(),
        ["no-input", "yes", "snapshot", "gist"]
    );
    assert_eq!(spec.summary, "alias of `get`");
    assert_eq!(spec.tool, Some("get"));
    assert_eq!(spec.section, Section::Read);
    assert_eq!(
        spec.positionals,
        [Positional {
            label: "ID",
            required: true,
            description: ""
        }]
        .as_slice()
    );
}

/// `inspect` renders today's usage text and builds today's parser sets.
#[test]
fn inspect_matches_the_registered_usage_and_parser() {
    let spec = record("inspect");
    assert_eq!(
        spec.full_usage(),
        "usage: clio inspect [--filter JSON] [--category C] [--epistemic-kind K] [--validity V] [--time-start TS] [--time-end TS] [--limit N] [--offset N] [--include-discarded] [--output json|text]",
        "inspect full usage"
    );
    assert_eq!(
        spec.usage_line(),
        "clio inspect [--filter JSON] [--category C] [--epistemic-kind K] [--validity V] [--time-start TS] [--time-end TS] [--limit N] [--offset N] [--include-discarded] [--output json|text]",
        "inspect usage line"
    );
    assert_eq!(
        spec.parser_spec().value_flags(),
        [
            "db",
            "backend",
            "bank",
            "actor",
            "output",
            "filter",
            "category",
            "epistemic-kind",
            "validity",
            "time-start",
            "time-end",
            "limit",
            "offset"
        ]
    );
    assert_eq!(
        spec.parser_spec().bool_flags(),
        ["no-input", "yes", "include-discarded"]
    );
    assert_eq!(spec.summary, "metadata-only listing, no decrypted body");
    assert_eq!(spec.tool, Some("inspect"));
    assert_eq!(spec.section, Section::Read);
    assert!(spec.positionals.is_empty(), "{:?}", spec.positionals);
}

/// `stats` renders today's usage text and builds today's parser sets.
#[test]
fn stats_matches_the_registered_usage_and_parser() {
    let spec = record("stats");
    assert_eq!(
        spec.full_usage(),
        "usage: clio stats [--bank BANK] [--output json|text]",
        "stats full usage"
    );
    assert_eq!(
        spec.usage_line(),
        "clio stats [--bank BANK] [--output json|text]",
        "stats usage line"
    );
    assert_eq!(
        spec.parser_spec().value_flags(),
        ["db", "backend", "bank", "actor", "output"]
    );
    assert_eq!(spec.parser_spec().bool_flags(), ["no-input", "yes"]);
    assert_eq!(
        spec.summary,
        "count memories, entities, and index state in one bank"
    );
    assert_eq!(spec.tool, Some("stats"));
    assert_eq!(spec.section, Section::Read);
    assert!(spec.positionals.is_empty(), "{:?}", spec.positionals);
}

/// The five records carry no aliases, overrides, alternates, or examples yet;
/// only `recall` carries prose. This pins that none of them grew quietly.
#[test]
fn the_five_records_carry_no_extra_surfaces() {
    for name in ["recall", "get", "show", "inspect", "stats"] {
        let spec = record(name);
        assert!(spec.aliases.is_empty(), "{name} aliases");
        assert!(spec.flag_aliases.is_empty(), "{name} flag aliases");
        assert!(spec.usage_override.is_none(), "{name} override");
        assert!(spec.usage_alternates.is_empty(), "{name} alternates");
        assert!(spec.examples.is_empty(), "{name} examples");
        assert!(spec.required_flags().is_empty(), "{name} required flags");
    }
    assert_eq!(record("recall").notes.len(), 1, "recall notes");
    for name in ["get", "show", "inspect", "stats"] {
        assert!(record(name).notes.is_empty(), "{name} notes");
    }
}
