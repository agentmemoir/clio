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
//! - The tranche's flag metadata: the enum lists the consumers accept and the
//!   value kinds their parsers use
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

use crate::cli_command_registry::check_metadata;
use crate::cli_command_registry_test_support::{positionals, record};
use crate::cli_command_spec::{CommandSpec, Section, ValueKind};

/// `recall` renders today's usage text and builds today's parser sets.
#[test]
fn recall_matches_the_registered_usage_and_parser() {
    let spec = record("recall");
    assert_eq!(
        spec.full_usage(),
        "usage: clio recall QUERY [--bank BANK] [--limit N] [--domains a,b] [--as-of TS] [--query-anchor TS] [--time-axis valid|transaction] [--min-semantic N] [--min-keyword N] [--min-reranker N] [--min-final N] [--no-entities] [--entity-summary] [--output json|text]\n       a score floor is a relative filter over this query's own scores, not a calibrated threshold; --min-semantic and --min-keyword prune one retrieval arm each, and --no-entities cannot be combined with --entity-summary; --query-anchor must be RFC 3339 with an explicit UTC offset, and a relative phrase without an anchor contributes nothing (no server-time fallback)",
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
    assert_eq!(positionals(spec), [("QUERY", true)]);
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
    assert_eq!(spec.aliases, ["show"], "get declares its visible alias");
    assert_eq!(positionals(spec), [("ID", true)]);
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
    assert_eq!(positionals(spec), [("ID", true)]);
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

/// The records carry no flag aliases, overrides, alternates, or required
/// flags; `get` alone declares the `show` alias, every record carries staged
/// examples, and only `recall` carries prose. This pins that none of them
/// grew quietly.
#[test]
fn the_five_records_carry_no_extra_surfaces() {
    for name in ["recall", "get", "show", "inspect", "stats"] {
        let spec = record(name);
        assert!(spec.flag_aliases.is_empty(), "{name} flag aliases");
        assert!(spec.usage_override.is_none(), "{name} override");
        assert!(spec.usage_alternates.is_empty(), "{name} alternates");
        assert!(!spec.examples.is_empty(), "{name} examples");
        assert!(spec.required_flags().is_empty(), "{name} required flags");
    }
    assert_eq!(record("get").aliases, ["show"], "get aliases");
    for name in ["recall", "show", "inspect", "stats"] {
        assert!(record(name).aliases.is_empty(), "{name} aliases");
    }
    assert_eq!(record("recall").notes.len(), 1, "recall notes");
    for name in ["get", "show", "inspect", "stats"] {
        assert!(record(name).notes.is_empty(), "{name} notes");
    }
}

/// The staged example counts: every record carries the authored list, pinned
/// so a dropped example fails here as well as in the central contract.
#[test]
fn the_five_records_stage_their_examples() {
    for (name, count) in [
        ("recall", 3),
        ("get", 3),
        ("show", 3),
        ("inspect", 3),
        ("stats", 2),
    ] {
        assert_eq!(record(name).examples.len(), count, "{name} examples");
    }
}

/// A lookup for a command no record names fails saying which command is missing.
#[test]
#[should_panic(expected = "`no-such-command` is registered")]
fn record_panics_naming_the_missing_command() {
    let _ = record("no-such-command");
}

/// Every documented flag in this tranche carries a description and, when it
/// takes a value, a placeholder and non-empty kind.
#[test]
fn the_core_reads_tranche_passes_the_metadata_check() {
    let specs: Vec<&CommandSpec> = super::CORE_READS.iter().collect();
    assert_eq!(check_metadata(&specs), Ok(()));
}

/// `recall --limit` records the retrieve tool's constant default.
#[test]
fn recall_limit_metadata_default_is_the_retrieve_constant() {
    let limit = record("recall")
        .documented
        .iter()
        .find(|flag| flag.long == "limit")
        .expect("recall documents --limit");
    assert_eq!(
        limit
            .default
            .expect("--limit records a default")
            .parse::<usize>()
            .expect("the default is a number"),
        clio_retrieve::DEFAULT_LIMIT,
        "recall --limit default"
    );
}

/// Every documented flag carries the kind its consumer parses: the enum
/// lists name exactly the accepted values (`cli_read_args`'s two time axes
/// and `cli_read_core::inspect_call`'s four validity labels), the counted
/// flags are `Uint` (`parse_u64`), the score floors are `Float`
/// (`parse_f64`), and every remaining flag stays free text or the shared
/// `--output` global.
#[test]
fn the_core_reads_flag_kinds_match_their_consumers() {
    for spec in super::CORE_READS {
        for flag in spec.documented {
            let Some(value) = flag.value else { continue };
            let expected = match (spec.name, flag.long) {
                ("recall", "time-axis") => ValueKind::Enum(&["valid", "transaction"]),
                ("inspect", "validity") => {
                    ValueKind::Enum(&["all", "active", "invalidated", "discarded"])
                }
                ("recall" | "inspect", "limit") | ("inspect", "offset") => ValueKind::Uint,
                ("recall", "min-semantic" | "min-keyword" | "min-reranker" | "min-final") => {
                    ValueKind::Float
                }
                (_, "output") => ValueKind::Enum(&["json", "text"]),
                _ => ValueKind::Text,
            };
            assert_eq!(value.kind, expected, "`{} --{}`", spec.name, flag.long);
        }
    }
}

/// `inspect --limit` and `--offset` record the tool's defaults, which the
/// `inspect` tool applies inline (50 and 0), with no named constant.
#[test]
fn inspect_records_the_limit_and_offset_defaults_the_tool_applies() {
    let default = |flag: &str| {
        record("inspect")
            .documented
            .iter()
            .find(|candidate| candidate.long == flag)
            .unwrap_or_else(|| panic!("inspect documents `--{flag}`"))
            .default
    };
    assert_eq!(default("limit"), Some("50"), "inspect --limit default");
    assert_eq!(default("offset"), Some("0"), "inspect --offset default");
}
