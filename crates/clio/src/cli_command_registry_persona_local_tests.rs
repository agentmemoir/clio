// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the persona and local-state tranche: each record's bytes
//! against the registered usage text and the catalog row, and its exact
//! parser-visible flag sets.
//!
//! ## Owns
//! - Byte equality of each record's `full_usage` and `usage_line` with the
//!   usage text the shared lookup resolves
//! - The exact `parser_spec` value and boolean flag sequences per command
//! - Each record's summary, tool, section, positional values, and required bits
//! - The tranche's `validate` contract against the catalog inventory
//!
//! ## Does not own
//! - The registry lookups and `validate` itself (`cli_command_registry_tests`)
//! - The renderer's shapes over pure records (`cli_command_spec_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the persona and local-state records' data.

use crate::cli_command_registry_test_support::record;
use crate::cli_command_spec::{CommandSpec, Positional, Section};

/// The tranche's records, as the validation contract consumes them.
fn tranche() -> Vec<&'static CommandSpec> {
    super::PERSONA_LOCAL.iter().collect()
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
fn the_persona_local_tranche_validates_cleanly() {
    let inventory: Vec<&str> = crate::command_catalog::catalog_commands().collect();
    assert_eq!(
        crate::cli_command_registry::validate(&tranche(), &inventory),
        Ok(())
    );
}

/// The tranche lists its nine records once each, in the declared order.
#[test]
fn the_tranche_lists_its_nine_records_in_declared_order() {
    let names: Vec<&str> = super::PERSONA_LOCAL.iter().map(|spec| spec.name).collect();
    assert_eq!(
        names,
        [
            "persona get",
            "persona stable",
            "persona observe",
            "scratchpad write",
            "scratchpad read",
            "scratchpad clear",
            "canonical put",
            "canonical get",
            "maintenance status"
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

/// `persona get` renders today's usage text and builds today's parser sets.
#[test]
fn persona_get_matches_the_registered_usage_and_parser() {
    let spec = record("persona get");
    assert_eq!(
        spec.full_usage(),
        "usage: clio persona get [--budget-tokens N] [--explain] [--output json|text]"
    );
    assert_eq!(
        spec.usage_line(),
        "clio persona get [--budget-tokens N] [--explain] [--output json|text]"
    );
    assert_eq!(
        spec.parser_spec().value_flags(),
        ["db", "backend", "bank", "actor", "output", "budget-tokens"]
    );
    assert_eq!(
        spec.parser_spec().bool_flags(),
        ["no-input", "yes", "explain"]
    );
    assert_eq!(spec.summary, "the persona model, within a token budget");
    assert_eq!(spec.tool, Some("persona_get"));
    assert_eq!(spec.section, Section::Read);
    assert!(spec.positionals.is_empty(), "{:?}", spec.positionals);
}

/// `persona stable` renders today's usage text and builds today's parser sets.
#[test]
fn persona_stable_matches_the_registered_usage_and_parser() {
    let spec = record("persona stable");
    assert_eq!(
        spec.full_usage(),
        "usage: clio persona stable KEY VALUE [--confidence N] [--source explicit|inferred] [--evidence-ref E] [--context TEXT] [--dry-run] [--output json|text]"
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
            "source",
            "evidence-ref",
            "context"
        ]
    );
    assert_eq!(
        spec.parser_spec().bool_flags(),
        ["no-input", "yes", "dry-run"]
    );
    assert_eq!(spec.summary, "set a stable persona key");
    assert_eq!(spec.tool, Some("persona_put_stable"));
    assert_eq!(spec.section, Section::Write);
    assert_eq!(
        spec.positionals,
        [positional("KEY", true), positional("VALUE", true)].as_slice()
    );
}

/// `persona observe` renders today's usage text while still accepting the
/// `--context` its printed line never mentions.
#[test]
fn persona_observe_matches_the_registered_usage_and_parser() {
    let spec = record("persona observe");
    assert_eq!(
        spec.full_usage(),
        "usage: clio persona observe KEY NUMBER [--confidence N] [--source explicit|inferred] [--evidence-ref E] [--output json|text]"
    );
    assert_eq!(
        spec.usage_override,
        Some(
            "clio persona observe KEY NUMBER [--confidence N] [--source explicit|inferred] [--evidence-ref E] [--output json|text]"
        ),
        "the override is what keeps --context out of the printed line"
    );
    assert!(
        !spec.full_usage().contains("--context"),
        "{}",
        spec.full_usage()
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
            "source",
            "evidence-ref",
            "context"
        ]
    );
    assert_eq!(spec.parser_spec().bool_flags(), ["no-input", "yes"]);
    assert_eq!(spec.summary, "feed one numeric observation into the EMA");
    assert_eq!(spec.tool, Some("persona_observe_preference"));
    assert_eq!(spec.section, Section::Write);
    assert_eq!(
        spec.positionals,
        [positional("KEY", true), positional("NUMBER", true)].as_slice()
    );
}

/// `scratchpad write` renders today's usage text and builds today's parser sets.
#[test]
fn scratchpad_write_matches_the_registered_usage_and_parser() {
    let spec = record("scratchpad write");
    assert_eq!(
        spec.full_usage(),
        "usage: clio scratchpad write KEY CONTENT [--session S] [--bank B] [--actor A] [--output json|text]"
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
        spec.positionals,
        [positional("KEY", true), positional("CONTENT", true)].as_slice()
    );
}

/// `scratchpad read` renders today's usage text and builds today's parser sets.
#[test]
fn scratchpad_read_matches_the_registered_usage_and_parser() {
    let spec = record("scratchpad read");
    assert_eq!(
        spec.full_usage(),
        "usage: clio scratchpad read [KEY] [--session S] [--bank B] [--actor A] [--output json|text]"
    );
    assert_eq!(
        spec.parser_spec().value_flags(),
        ["db", "backend", "bank", "actor", "output", "session"]
    );
    assert_eq!(spec.parser_spec().bool_flags(), ["no-input", "yes"]);
    assert_eq!(spec.summary, "read one key, or the whole pad");
    assert_eq!(spec.tool, Some("scratchpad_read"));
    assert_eq!(spec.section, Section::Read);
    assert_eq!(spec.positionals, [positional("KEY", false)].as_slice());
}

/// `scratchpad clear` renders today's usage text and builds today's parser sets.
#[test]
fn scratchpad_clear_matches_the_registered_usage_and_parser() {
    let spec = record("scratchpad clear");
    assert_eq!(
        spec.full_usage(),
        "usage: clio scratchpad clear [KEY] [--session S] [--bank B] [--actor A] [--output json|text]"
    );
    assert_eq!(
        spec.parser_spec().value_flags(),
        ["db", "backend", "bank", "actor", "output", "session"]
    );
    assert_eq!(spec.parser_spec().bool_flags(), ["no-input", "yes"]);
    assert_eq!(spec.summary, "drop one key, or the whole pad");
    assert_eq!(spec.tool, Some("scratchpad_clear"));
    assert_eq!(spec.section, Section::Write);
    assert_eq!(spec.positionals, [positional("KEY", false)].as_slice());
}

/// `canonical put` renders today's usage text and carries the required bits
/// its refusal path already reads by verb name.
#[test]
fn canonical_put_matches_the_registered_usage_and_parser() {
    let spec = record("canonical put");
    assert_eq!(
        spec.full_usage(),
        "usage: clio canonical put KEY VALUE --category C --epistemic-kind K [--source-type T] [--confidence N] [--source-text TEXT] [--evidence-ref E] [--context C] [--dry-run] [--output json|text]"
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
        spec.positionals,
        [positional("KEY", true), positional("VALUE", true)].as_slice()
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
    assert_eq!(spec.positionals, [positional("KEY", true)].as_slice());
}

/// `maintenance status` renders today's usage text and builds today's parser sets.
#[test]
fn maintenance_status_matches_the_registered_usage_and_parser() {
    let spec = record("maintenance status");
    assert_eq!(
        spec.full_usage(),
        "usage: clio maintenance status [--item-id ID] [--explain] [--output json|text]"
    );
    assert_eq!(
        spec.parser_spec().value_flags(),
        ["db", "backend", "bank", "actor", "output", "item-id"]
    );
    assert_eq!(
        spec.parser_spec().bool_flags(),
        ["no-input", "yes", "explain"]
    );
    assert_eq!(spec.summary, "queue depth, failures, and drain progress");
    assert_eq!(spec.tool, Some("maintenance_status"));
    assert_eq!(spec.section, Section::Read);
    assert!(spec.positionals.is_empty(), "{:?}", spec.positionals);
}
