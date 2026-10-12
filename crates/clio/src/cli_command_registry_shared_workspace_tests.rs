// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the shared-bank, intent, compose, and audit records, and
//! the tranche-level contract over all nine `SHARED_WORKSPACE` records: byte
//! equality against the registered usage text, the exact parser-visible flag
//! sets, the `validate` contract, and the flag metadata. The hygiene
//! per-record tests live in the `hygiene` sibling's test module.
//!
//! ## Owns
//! - Byte equality of each shared, intent, compose, and audit record's
//!   `full_usage` and `usage_line`, and its exact `parser_spec` flag sets
//! - The tranche list order, the `validate` contract, and the flag metadata
//!   over the whole tranche
//! - Each of these records' summary, tool, section, positional shape, and
//!   authored example count
//!
//! ## Does not own
//! - The hygiene usage and parser pins (see the `hygiene` sibling's `tests`
//!   module)
//! - The registry lookups and `validate` itself (`cli_command_registry_tests`)
//! - The renderer's shapes over pure records (`cli_command_spec_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the shared-bank, intent, compose, and audit
//! records and the tranche-level checks.

use crate::cli_command_registry::check_metadata;
use crate::cli_command_registry_test_support::{positionals, record};
use crate::cli_command_spec::{CommandSpec, Section, ValueKind};

/// The tranche's records, as the validation contract consumes them.
fn tranche() -> Vec<&'static CommandSpec> {
    super::SHARED_WORKSPACE.iter().collect()
}

/// The tranche's records pass `validate` against the catalog inventory.
#[test]
fn the_shared_workspace_tranche_validates_cleanly() {
    let inventory: Vec<&str> = crate::command_catalog::catalog_commands().collect();
    assert_eq!(
        crate::cli_command_registry::validate(&tranche(), &inventory),
        Ok(())
    );
}

/// The tranche lists its nine records once each, in the declared order.
#[test]
fn the_tranche_lists_its_nine_records_in_declared_order() {
    let names: Vec<&str> = super::SHARED_WORKSPACE
        .iter()
        .map(|spec| spec.name)
        .collect();
    assert_eq!(
        names,
        [
            "shared store",
            "shared retrieve",
            "shared discard",
            "hygiene audit",
            "hygiene clean",
            "hygiene log",
            "intent gate",
            "compose-context",
            "audit trail"
        ]
    );
}

/// Every record answers the shared lookup with its own bytes and a summary,
/// so the authored rows can be deleted without changing either surface.
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
            "{} resolves through the shared lookup",
            spec.name
        );
        assert!(!spec.summary.is_empty(), "{} summary", spec.name);
    }
}

/// `shared store` renders today's usage text and builds today's parser sets.
#[test]
fn shared_store_matches_the_registered_usage_and_parser() {
    let spec = record("shared store");
    assert_eq!(
        spec.full_usage(),
        "usage: clio shared store TEXT --epistemic-kind K [--category C] [--episodic-type T] [--source-type T] [--confidence N] [--evidence-ref E] --actor ACTOR --shared-bank [--output json|text]\n       --epistemic-kind and --source-type are a pair: --source-type (user_stated|agent_inferred|third_party) is required with belief and must be omitted with fact"
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
            "episodic-type",
            "epistemic-kind",
            "source-type",
            "confidence",
            "evidence-ref",
        ]
    );
    assert_eq!(
        spec.parser_spec().bool_flags(),
        ["no-input", "yes", "shared-bank"]
    );
    assert_eq!(
        spec.required_flags(),
        ["epistemic-kind", "actor", "shared-bank"]
    );
    assert_eq!(spec.summary, "write into a shared bank");
    assert_eq!(spec.tool, Some("shared_store"));
    assert_eq!(spec.section, Section::Write);
    assert_eq!(
        positionals(spec),
        [("TEXT", true)],
        "shared store positionals"
    );
}

/// `shared retrieve` renders today's usage text and builds today's parser sets.
#[test]
fn shared_retrieve_matches_the_registered_usage_and_parser() {
    let spec = record("shared retrieve");
    assert_eq!(
        spec.full_usage(),
        "usage: clio shared retrieve QUERY --actor ACTOR --shared-bank [--output json|text]"
    );
    assert_eq!(
        spec.parser_spec().value_flags(),
        ["db", "backend", "bank", "actor", "output"]
    );
    assert_eq!(
        spec.parser_spec().bool_flags(),
        ["no-input", "yes", "shared-bank"]
    );
    assert_eq!(spec.required_flags(), ["actor", "shared-bank"]);
    assert_eq!(spec.summary, "read from a shared bank");
    assert_eq!(spec.tool, Some("shared_retrieve"));
    assert_eq!(spec.section, Section::Read);
    assert_eq!(
        positionals(spec),
        [("QUERY", true)],
        "shared retrieve positionals"
    );
}

/// `shared discard` renders today's usage text and builds today's parser sets.
#[test]
fn shared_discard_matches_the_registered_usage_and_parser() {
    let spec = record("shared discard");
    assert_eq!(
        spec.full_usage(),
        "usage: clio shared discard ITEM_ID --reason R [--confirm] --actor ACTOR --shared-bank [--output json|text]"
    );
    assert_eq!(
        spec.parser_spec().value_flags(),
        ["db", "backend", "bank", "actor", "output", "reason"]
    );
    assert_eq!(
        spec.parser_spec().bool_flags(),
        ["no-input", "yes", "confirm", "shared-bank"]
    );
    assert_eq!(spec.required_flags(), ["reason", "actor", "shared-bank"]);
    assert_eq!(spec.summary, "remove one item from a shared bank");
    assert_eq!(spec.tool, Some("shared_discard"));
    assert_eq!(spec.section, Section::Write);
    assert_eq!(
        positionals(spec),
        [("ITEM_ID", true)],
        "shared discard positionals"
    );
}

/// `intent gate` renders today's usage text and builds today's parser sets.
#[test]
fn intent_gate_matches_the_registered_usage_and_parser() {
    let spec = record("intent gate");
    assert_eq!(
        spec.full_usage(),
        "usage: clio intent gate TURN_TEXT [--explain] [--output json|text]"
    );
    assert_eq!(
        spec.parser_spec().value_flags(),
        ["db", "backend", "bank", "actor", "output"]
    );
    assert_eq!(
        spec.parser_spec().bool_flags(),
        ["no-input", "yes", "explain"]
    );
    assert_eq!(spec.summary, "decide whether a turn should reach memory");
    assert_eq!(spec.tool, Some("intent_gate"));
    assert_eq!(spec.section, Section::Read);
    assert_eq!(
        positionals(spec),
        [("TURN_TEXT", true)],
        "intent gate positionals"
    );
}

/// `compose-context` accepts a `--query` its printed line never shows and
/// prints `--budget-tokens` bare, so the line is pinned while the parser set
/// carries both.
#[test]
fn compose_context_matches_the_registered_usage_and_parser() {
    let spec = record("compose-context");
    assert_eq!(
        spec.full_usage(),
        "usage: clio compose-context [QUERY] --budget-tokens N [--domains a,b] [--as-of TS] [--query-anchor TS] [--time-axis valid|transaction] [--recall-scope full|consolidated_only] [--expand-graph] [--prefer-consolidated] [--explain] [--output json|text]"
    );
    assert!(
        !spec.full_usage().contains("[--query Q]"),
        "the hidden --query flag must not print: {}",
        spec.full_usage()
    );
    assert!(
        spec.required_flags().is_empty(),
        "budget-tokens is mandated but not a required-flag bit: {:?}",
        spec.required_flags()
    );
    assert_eq!(
        spec.parser_spec().value_flags(),
        [
            "db",
            "backend",
            "bank",
            "actor",
            "output",
            "query",
            "domains",
            "budget-tokens",
            "as-of",
            "query-anchor",
            "time-axis",
            "recall-scope"
        ]
    );
    assert_eq!(
        spec.parser_spec().bool_flags(),
        [
            "no-input",
            "yes",
            "expand-graph",
            "prefer-consolidated",
            "explain"
        ]
    );
    assert_eq!(spec.summary, "assemble a budgeted context block for a turn");
    assert_eq!(spec.tool, Some("compose_context"));
    assert_eq!(spec.section, Section::Read);
    assert_eq!(
        positionals(spec),
        [("QUERY", false)],
        "compose-context positionals"
    );
}

/// `audit trail` renders today's usage text and builds today's parser sets.
#[test]
fn audit_trail_matches_the_registered_usage_and_parser() {
    let spec = record("audit trail");
    assert_eq!(
        spec.full_usage(),
        "usage: clio audit trail ITEM_ID [--explain] [--output json|text]"
    );
    assert_eq!(
        spec.parser_spec().value_flags(),
        ["db", "backend", "bank", "actor", "output"]
    );
    assert_eq!(
        spec.parser_spec().bool_flags(),
        ["no-input", "yes", "explain"]
    );
    assert_eq!(spec.summary, "masked lifecycle reconstruction for one item");
    assert_eq!(spec.tool, Some("audit_trail"));
    assert_eq!(spec.section, Section::Read);
    assert_eq!(
        positionals(spec),
        [("ITEM_ID", true)],
        "audit trail positionals"
    );
}

/// The shared, intent, compose, and audit records carry the authored example
/// counts, pinned here so an empty or shrunken list fails the tranche tests,
/// not only the metadata check. The hygiene counts are pinned by the sibling's
/// test module.
#[test]
fn the_shared_workspace_records_carry_their_examples() {
    for (name, count) in [
        ("shared store", 3),
        ("shared retrieve", 2),
        ("shared discard", 2),
        ("intent gate", 2),
        ("compose-context", 3),
        ("audit trail", 2),
    ] {
        assert_eq!(record(name).examples.len(), count, "{name} examples");
    }
}

/// Every documented flag in this tranche carries a description and, when it
/// takes a value, a placeholder and non-empty kind.
#[test]
fn the_shared_workspace_tranche_passes_the_metadata_check() {
    let specs: Vec<&CommandSpec> = super::SHARED_WORKSPACE.iter().collect();
    assert_eq!(check_metadata(&specs), Ok(()));
}

/// Every documented flag carries the kind its consumer parses: the enum
/// lists are `cli_write_shared`'s two epistemic kinds, `SourceType`'s wire
/// table, `cli_write_hygiene`'s five actions, `validate_time_axis`'s two
/// axes, and the `compose_context` schema's two scopes; the counted flags
/// are `Uint`, the unit-bounded and score floors `Float`, and every
/// remaining flag stays free text or the shared `--output` global.
#[test]
fn the_shared_workspace_flag_kinds_match_their_consumers() {
    let actions = ValueKind::Enum(&["flag", "archive", "discard", "delete", "keep"]);
    for spec in super::SHARED_WORKSPACE {
        for flag in spec.documented {
            let Some(value) = flag.value else { continue };
            let expected = match (spec.name, flag.long) {
                ("shared store", "epistemic-kind") => ValueKind::Enum(&["fact", "belief"]),
                ("shared store", "source-type") => ValueKind::Enum(clio_types::SourceType::WIRE),
                ("hygiene clean", "action") => actions,
                ("compose-context", "time-axis") => ValueKind::Enum(&["valid", "transaction"]),
                ("compose-context", "recall-scope") => {
                    ValueKind::Enum(&["full", "consolidated_only"])
                }
                ("shared store", "confidence") | ("hygiene audit", "min-score") => ValueKind::Float,
                ("hygiene audit" | "hygiene log", "limit" | "offset")
                | ("compose-context", "budget-tokens") => ValueKind::Uint,
                (_, "output") => ValueKind::Enum(&["json", "text"]),
                _ => ValueKind::Text,
            };
            assert_eq!(value.kind, expected, "`{} --{}`", spec.name, flag.long);
        }
    }
}
