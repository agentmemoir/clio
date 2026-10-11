// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the shared-workspace tranche: each record's bytes against
//! the registered usage text and the catalog row, and its exact parser-visible
//! flag sets, the `hygiene clean` alternate and note included.
//!
//! ## Owns
//! - Byte equality of each record's `full_usage` and `usage_line` with the
//!   usage text the shared lookup resolves
//! - The exact `parser_spec` value and boolean flag sequences per command
//! - Each record's summary, tool, section, positional values, and required bits
//! - The tranche's `validate` contract against the catalog inventory
//! - The tranche's flag metadata: the enum lists the consumers accept and the
//!   value kinds their parsers use
//!
//! ## Does not own
//! - The registry lookups and `validate` itself (`cli_command_registry_tests`)
//! - The renderer's shapes over pure records (`cli_command_spec_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the shared-workspace records' data.

use crate::cli_command_registry::check_metadata;
use crate::cli_command_registry_test_support::record;
use crate::cli_command_spec::{CommandSpec, Positional, Section, ValueKind};

/// The tranche's records, as the validation contract consumes them.
fn tranche() -> Vec<&'static CommandSpec> {
    super::SHARED_WORKSPACE.iter().collect()
}

/// A positional literal: these records carry no positional description.
const fn positional(label: &'static str, required: bool) -> Positional {
    Positional {
        label,
        required,
        description: "",
    }
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
        "usage: clio shared store TEXT --epistemic-kind K [--category C] [--episodic-type T] [--source-type T] [--confidence N] [--evidence-ref E] --actor A --shared-bank [--output json|text]\n       --epistemic-kind and --source-type are a pair: --source-type (user_stated|agent_inferred|third_party) is required with belief and must be omitted with fact"
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
    assert_eq!(spec.positionals, [positional("TEXT", true)].as_slice());
}

/// `shared retrieve` renders today's usage text and builds today's parser sets.
#[test]
fn shared_retrieve_matches_the_registered_usage_and_parser() {
    let spec = record("shared retrieve");
    assert_eq!(
        spec.full_usage(),
        "usage: clio shared retrieve QUERY --actor A --shared-bank [--output json|text]"
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
    assert_eq!(spec.positionals, [positional("QUERY", true)].as_slice());
}

/// `shared discard` renders today's usage text and builds today's parser sets.
#[test]
fn shared_discard_matches_the_registered_usage_and_parser() {
    let spec = record("shared discard");
    assert_eq!(
        spec.full_usage(),
        "usage: clio shared discard ITEM_ID --reason R [--confirm] --actor A --shared-bank [--output json|text]"
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
    assert_eq!(spec.positionals, [positional("ITEM_ID", true)].as_slice());
}

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
    assert_eq!(spec.positionals, [positional("TURN_TEXT", true)].as_slice());
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
    assert_eq!(spec.positionals, [positional("QUERY", false)].as_slice());
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
    assert_eq!(spec.positionals, [positional("ITEM_ID", true)].as_slice());
}

/// Every documented flag in this tranche carries a description and, when it
/// takes a value, a placeholder and non-empty kind.
#[test]
fn the_shared_workspace_tranche_passes_the_metadata_check() {
    let specs: Vec<&CommandSpec> = super::SHARED_WORKSPACE.iter().collect();
    assert_eq!(check_metadata(&specs), Ok(()));
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
