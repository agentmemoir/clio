// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the persona and maintenance records, and the tranche-level
//! contract over all nine `PERSONA_LOCAL` records: byte equality against the
//! registered usage text, the exact parser-visible flag sets, the tranche's
//! `validate` contract, and the flag metadata. The scratchpad and canonical
//! per-record tests live in the `workspace` sibling's test module.
//!
//! ## Owns
//! - Byte equality of each persona and maintenance record's `full_usage` and
//!   `usage_line`, and its exact `parser_spec` flag sets
//! - The tranche list order, the `validate` contract, the alias and flag-alias
//!   emptiness, and the flag metadata over the whole tranche
//! - Each persona and maintenance record's summary, tool, section, positional
//!   shape, and authored example count
//!
//! ## Does not own
//! - The scratchpad and canonical usage and parser pins (see the `workspace`
//!   sibling's `tests` module)
//! - The registry lookups and `validate` itself (`cli_command_registry_tests`)
//! - The renderer's shapes over pure records (`cli_command_spec_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the persona and maintenance records and the
//! tranche-level checks.

use crate::cli_command_registry_test_support::{positionals, record};
use crate::cli_command_spec::{CommandSpec, Section, ValueKind};

/// One record's documented flag, as the metadata tests read it.
fn documented_flag(name: &str, flag: &str) -> &'static crate::cli_command_spec::Flag {
    record(name)
        .documented
        .iter()
        .find(|candidate| candidate.long == flag)
        .unwrap_or_else(|| panic!("`{name}` documents `--{flag}`"))
}

/// The tranche's records, as the validation contract consumes them.
fn tranche() -> Vec<&'static CommandSpec> {
    super::PERSONA_LOCAL.iter().collect()
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
        "usage: clio persona get [--budget-tokens N] [--explain] [--bank BANK] [--output json|text]"
    );
    assert_eq!(
        spec.usage_line(),
        "clio persona get [--budget-tokens N] [--explain] [--bank BANK] [--output json|text]"
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
        "usage: clio persona stable KEY VALUE [--confidence N] [--source explicit|inferred] [--evidence-ref E] [--context TEXT] [--dry-run] [--bank BANK] [--output json|text]"
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
        positionals(spec),
        [("KEY", true), ("VALUE", true)],
        "persona stable positionals"
    );
}

/// `persona observe` renders today's usage text while still accepting the
/// `--context` its printed line never mentions.
#[test]
fn persona_observe_matches_the_registered_usage_and_parser() {
    let spec = record("persona observe");
    assert_eq!(
        spec.full_usage(),
        "usage: clio persona observe KEY NUMBER [--confidence N] [--source explicit|inferred] [--evidence-ref E] [--bank BANK] [--output json|text]"
    );
    assert_eq!(
        spec.usage_override,
        Some(
            "clio persona observe KEY NUMBER [--confidence N] [--source explicit|inferred] [--evidence-ref E] [--bank BANK] [--output json|text]"
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
        positionals(spec),
        [("KEY", true), ("NUMBER", true)],
        "persona observe positionals"
    );
}

/// `maintenance status` renders today's usage text and builds today's parser sets.
#[test]
fn maintenance_status_matches_the_registered_usage_and_parser() {
    let spec = record("maintenance status");
    assert_eq!(
        spec.full_usage(),
        "usage: clio maintenance status [--item-id ID] [--explain] [--bank BANK] [--output json|text]"
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

/// The persona and maintenance records carry the authored example counts,
/// pinned here so an empty or shrunken list fails the tranche tests, not only
/// the metadata check. The scratchpad and canonical counts are pinned by the
/// sibling's test module.
#[test]
fn the_persona_and_maintenance_records_carry_their_examples() {
    for (name, count) in [
        ("persona get", 3),
        ("persona stable", 3),
        ("persona observe", 3),
        ("maintenance status", 2),
    ] {
        assert_eq!(record(name).examples.len(), count, "{name} examples");
    }
}

/// Every `PERSONA_LOCAL` record declares no aliases and no flag aliases, as
/// the sibling tranches pin: an alias added here fails this sweep.
#[test]
fn the_persona_local_records_declare_no_aliases() {
    for spec in super::PERSONA_LOCAL {
        assert!(spec.aliases.is_empty(), "{} aliases", spec.name);
        assert!(spec.flag_aliases.is_empty(), "{} flag aliases", spec.name);
    }
}

/// Every documented flag in this tranche carries a description and, for a value
/// flag, a placeholder and a non-empty kind.
#[test]
fn the_persona_local_flag_metadata_is_complete() {
    assert_eq!(
        crate::cli_command_registry::check_metadata(&tranche()),
        Ok(())
    );
}

/// Every documented flag carries the kind its consumer parses: the enum
/// lists name exactly the accepted values (`cli_write_persona`'s two sources
/// and the `clio_types` wire tables for categories and provenance), the
/// counted flag is `Uint`, the unit-bounded confidences are `Float`, and
/// every remaining flag stays free text or the shared `--output` global.
#[test]
fn the_persona_local_flag_kinds_match_their_consumers() {
    for spec in super::PERSONA_LOCAL {
        for flag in spec.documented {
            let Some(value) = flag.value else { continue };
            let expected = match (spec.name, flag.long) {
                ("persona stable" | "persona observe", "source") => {
                    ValueKind::Enum(&["explicit", "inferred"])
                }
                ("canonical put", "category") => {
                    ValueKind::Enum(clio_types::SemanticCategory::WIRE)
                }
                ("canonical put", "epistemic-kind") => ValueKind::Enum(&["fact", "belief"]),
                ("canonical put", "source-type") => ValueKind::Enum(clio_types::SourceType::WIRE),
                ("persona get", "budget-tokens") => ValueKind::Uint,
                ("persona stable" | "persona observe" | "canonical put", "confidence") => {
                    ValueKind::Float
                }
                (_, "output") => ValueKind::Enum(&["json", "text"]),
                _ => ValueKind::Text,
            };
            assert_eq!(value.kind, expected, "`{} --{}`", spec.name, flag.long);
        }
    }
}

/// `persona get --budget-tokens` records `clio_persona`'s constant default.
#[test]
fn persona_get_budget_tokens_default_is_the_persona_constant() {
    let budget = documented_flag("persona get", "budget-tokens")
        .default
        .expect("--budget-tokens records a default");
    assert_eq!(
        budget.parse::<usize>().expect("the default is a number"),
        clio_persona::DEFAULT_PERSONA_TOKEN_BUDGET,
        "persona get --budget-tokens default"
    );
}
