// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the memory-write tranche: each record's bytes against the
//! registered usage text, and its exact parser-visible flag sets, the `admit`
//! file alternate and the `remember`/`admit` note included.
//!
//! ## Owns
//! - Byte equality of each record's `full_usage` and `usage_line` with the
//!   usage text the write lookup resolves
//! - The exact `parser_spec` value and boolean flag sequences per command
//! - Each record's summary, tool, section, positional, and required-flag values
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
//! Keep this module focused on the memory-write records' data.

use crate::cli_command_registry::check_metadata;
use crate::cli_command_registry_test_support::{positionals, record};
use crate::cli_command_spec::{CommandSpec, Section, ValueKind};

/// The tranche's records, as the validation contract consumes them.
fn tranche() -> Vec<&'static CommandSpec> {
    super::CORE_WRITES.iter().collect()
}

/// The tranche's records pass `validate` against the catalog inventory.
#[test]
fn the_core_writes_tranche_validates_cleanly() {
    let inventory: Vec<&str> = crate::command_catalog::catalog_commands().collect();
    assert_eq!(
        crate::cli_command_registry::validate(&tranche(), &inventory),
        Ok(())
    );
}

/// The tranche lists its eight records once each, in the declared order.
#[test]
fn the_tranche_lists_its_eight_records_in_declared_order() {
    let names: Vec<&str> = super::CORE_WRITES.iter().map(|spec| spec.name).collect();
    assert_eq!(
        names,
        [
            "remember",
            "admit",
            "summarize",
            "consolidate",
            "update",
            "invalidate",
            "discard",
            "correct"
        ]
    );
}

/// Every record answers the write lookup with its own bytes, so the authored
/// rows can be deleted without changing either surface.
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
            "{} resolves through the write lookup",
            spec.name
        );
        assert_eq!(
            crate::cli_help_usage::usage_line(spec.name),
            Some(spec.usage_line()),
            "{} usage line resolves through the lookup",
            spec.name
        );
    }
    // `remember` renders no description: its authored catalog row was empty,
    // so its record carries an empty summary too.
    for spec in &tranche() {
        if spec.name != "remember" {
            assert!(!spec.summary.is_empty(), "{} summary", spec.name);
        }
    }
}

/// `remember` renders today's usage text and builds today's parser sets.
#[test]
fn remember_matches_the_registered_usage_and_parser() {
    let spec = record("remember");
    assert_eq!(
        spec.full_usage(),
        "usage: clio remember TEXT --category C [--bank BANK] [--actor ACTOR] [--epistemic-kind fact|belief] [--confidence N] [--source-type T] [--evidence-ref E] [--context TEXT] [--allow-duplicate] [--dry-run] [--output json|text]\n       --context records where the memory came from, such as a conversation type or workflow stage; it is not searchable and recall does not match on it, so put anything you want to find later in TEXT. It is at most 4096 UTF-8 bytes, rejected when empty or oversized, never truncated\n       --epistemic-kind and --source-type are a pair: --source-type (user_stated|agent_inferred|third_party) is required with belief and must be omitted with fact",
        "remember full usage"
    );
    assert_eq!(
        spec.usage_line(),
        "clio remember TEXT --category C [--bank BANK] [--actor ACTOR] [--epistemic-kind fact|belief] [--confidence N] [--source-type T] [--evidence-ref E] [--context TEXT] [--allow-duplicate] [--dry-run] [--output json|text]",
        "remember usage line"
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
            "confidence",
            "source-type",
            "evidence-ref",
            "context",
        ]
    );
    assert_eq!(
        spec.parser_spec().bool_flags(),
        ["no-input", "yes", "allow-duplicate", "dry-run"]
    );
    assert_eq!(spec.required_flags(), ["category"]);
    assert_eq!(spec.summary, "", "the catalog row rendered no description");
    assert_eq!(spec.tool, Some("store"));
    assert_eq!(spec.section, Section::Write);
    assert_eq!(positionals(spec), [("TEXT", true)]);
}

/// `admit` pins its printed line, shows the `--file` alternate, and carries
/// the hidden `--file` flag only the alternate names.
#[test]
fn admit_renders_the_file_alternate_and_hides_the_file_flag() {
    let spec = record("admit");
    assert_eq!(
        spec.full_usage(),
        "usage: clio admit TEXT --category C [--bank BANK] [--actor ACTOR] [--epistemic-kind fact|belief] [--confidence N] [--source-type T] [--evidence-ref E] [--context TEXT] [--output json|text]\n       or: clio admit --file ops.json [--bank BANK] [--actor ACTOR]\n       --context records where the memory came from, such as a conversation type or workflow stage; it is not searchable and recall does not match on it, so put anything you want to find later in TEXT. It is at most 4096 UTF-8 bytes, rejected when empty or oversized, never truncated\n       --epistemic-kind and --source-type are a pair: --source-type (user_stated|agent_inferred|third_party) is required with belief and must be omitted with fact",
        "admit full usage"
    );
    assert_eq!(
        spec.usage_line(),
        "clio admit TEXT --category C [--bank BANK] [--actor ACTOR] [--epistemic-kind fact|belief] [--confidence N] [--source-type T] [--evidence-ref E] [--context TEXT] [--output json|text]",
        "admit usage line"
    );
    assert!(
        !spec.usage_line().contains("--file"),
        "the hidden --file flag must not print in the line: {}",
        spec.usage_line()
    );
    let or_at = spec
        .full_usage()
        .find("\n       or: ")
        .expect("the alternate prints");
    let note_at = spec
        .full_usage()
        .find("\n       --context")
        .expect("the note prints");
    assert!(or_at < note_at, "or before note");
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
            "confidence",
            "source-type",
            "evidence-ref",
            "context",
            "file"
        ]
    );
    assert_eq!(spec.parser_spec().bool_flags(), ["no-input", "yes"]);
    assert_eq!(spec.required_flags(), ["category"]);
    assert_eq!(
        spec.summary,
        "score one write without storing it; `--file ops.json` previews a whole batch instead"
    );
    assert_eq!(spec.tool, Some("admit_preview"));
    assert_eq!(spec.section, Section::Read, "admit stores nothing");
    assert_eq!(positionals(spec), [("TEXT", true)]);
}

/// `summarize` renders today's usage text and builds today's parser sets.
#[test]
fn summarize_matches_the_registered_usage_and_parser() {
    let spec = record("summarize");
    assert_eq!(
        spec.full_usage(),
        "usage: clio summarize --scope S [--dry-run] [--output json|text]",
        "summarize full usage"
    );
    assert_eq!(
        spec.parser_spec().value_flags(),
        ["db", "backend", "bank", "actor", "output", "scope"]
    );
    assert_eq!(
        spec.parser_spec().bool_flags(),
        ["no-input", "yes", "dry-run"]
    );
    assert_eq!(spec.required_flags(), ["scope"]);
    assert_eq!(spec.summary, "summarize one scope into a consolidated unit");
    assert_eq!(spec.tool, Some("summarize"));
    assert_eq!(spec.section, Section::Write);
    assert!(spec.positionals.is_empty(), "{:?}", spec.positionals);
}

/// `consolidate` renders today's usage text and its wave note.
#[test]
fn consolidate_matches_the_registered_usage_and_parser() {
    let spec = record("consolidate");
    assert_eq!(
        spec.full_usage(),
        "usage: clio consolidate [--bank BANK] [--force] [--dry-run] [--output json|text]\n       --bank selects the target bank for the maintenance wave; --force runs the full wave: refresh every dirty ancestor, then reconcile the bank's consolidated units",
        "consolidate full usage"
    );
    assert_eq!(
        spec.usage_line(),
        "clio consolidate [--bank BANK] [--force] [--dry-run] [--output json|text]",
        "consolidate usage line"
    );
    assert_eq!(
        spec.parser_spec().value_flags(),
        ["db", "backend", "bank", "actor", "output"]
    );
    assert_eq!(
        spec.parser_spec().bool_flags(),
        ["no-input", "yes", "force", "dry-run"]
    );
    assert!(
        spec.required_flags().is_empty(),
        "{:?}",
        spec.required_flags()
    );
    assert_eq!(spec.summary, "run the consolidation wave over a bank");
    assert_eq!(spec.tool, Some("consolidate"));
    assert_eq!(spec.section, Section::Write);
    assert!(spec.positionals.is_empty(), "{:?}", spec.positionals);
}

/// `update` renders today's usage text and builds today's parser sets.
#[test]
fn update_matches_the_registered_usage_and_parser() {
    let spec = record("update");
    assert_eq!(
        spec.full_usage(),
        "usage: clio update ID VALUE --rule discrete|continuous [--dry-run] [--bank BANK] [--actor ACTOR] [--output json|text]",
        "update full usage"
    );
    assert_eq!(
        spec.parser_spec().value_flags(),
        ["db", "backend", "bank", "actor", "output", "rule"]
    );
    assert_eq!(
        spec.parser_spec().bool_flags(),
        ["no-input", "yes", "dry-run"]
    );
    assert_eq!(spec.required_flags(), ["rule"]);
    assert_eq!(spec.summary, "set an item's value under a named rule");
    assert_eq!(spec.tool, Some("update"));
    assert_eq!(spec.section, Section::Write);
    assert_eq!(positionals(spec), [("ID", true), ("VALUE", true)]);
}

/// `invalidate` renders today's usage text and builds today's parser sets.
#[test]
fn invalidate_matches_the_registered_usage_and_parser() {
    let spec = record("invalidate");
    assert_eq!(
        spec.full_usage(),
        "usage: clio invalidate ID [--replacement ID] [--dry-run] [--bank BANK] [--actor ACTOR] [--output json|text]",
        "invalidate full usage"
    );
    assert_eq!(
        spec.parser_spec().value_flags(),
        ["db", "backend", "bank", "actor", "output", "replacement"]
    );
    assert_eq!(
        spec.parser_spec().bool_flags(),
        ["no-input", "yes", "dry-run"]
    );
    assert!(
        spec.required_flags().is_empty(),
        "{:?}",
        spec.required_flags()
    );
    assert_eq!(spec.summary, "close an item's validity; never deletes");
    assert_eq!(spec.tool, Some("invalidate"));
    assert_eq!(spec.section, Section::Write);
    assert_eq!(positionals(spec), [("ID", true)]);
}

/// `discard` renders today's usage text and builds today's parser sets.
#[test]
fn discard_matches_the_registered_usage_and_parser() {
    let spec = record("discard");
    assert_eq!(
        spec.full_usage(),
        "usage: clio discard ID --reason R --confirm [--dry-run] [--bank BANK] [--actor ACTOR] [--output json|text]",
        "discard full usage"
    );
    assert_eq!(
        spec.parser_spec().value_flags(),
        ["db", "backend", "bank", "actor", "output", "reason"]
    );
    assert_eq!(
        spec.parser_spec().bool_flags(),
        ["no-input", "yes", "confirm", "dry-run"]
    );
    assert_eq!(
        spec.required_flags(),
        ["reason"],
        "the confirm gate owns --confirm"
    );
    assert_eq!(
        spec.summary,
        "operational removal; distinct from compliance erasure"
    );
    assert_eq!(spec.tool, Some("discard"));
    assert_eq!(spec.section, Section::Write);
    assert_eq!(positionals(spec), [("ID", true)]);
}

/// `correct` pins its printed line and prints the `--confirm` flag that gates
/// the live run.
#[test]
fn correct_pins_the_line_and_prints_the_confirm_flag() {
    let spec = record("correct");
    assert_eq!(
        spec.full_usage(),
        "usage: clio correct ID VALUE --reason R [--context TEXT] [--confirm] [--dry-run] [--bank BANK] [--actor ACTOR] [--output json|text]",
        "correct full usage"
    );
    assert_eq!(
        spec.usage_line(),
        "clio correct ID VALUE --reason R [--context TEXT] [--confirm] [--dry-run] [--bank BANK] [--actor ACTOR] [--output json|text]",
        "correct usage line"
    );
    assert!(
        spec.usage_line().contains("[--confirm]"),
        "the gating --confirm flag must print in the line: {}",
        spec.usage_line()
    );
    assert_eq!(
        spec.parser_spec().value_flags(),
        [
            "db", "backend", "bank", "actor", "output", "reason", "context"
        ]
    );
    assert_eq!(
        spec.parser_spec().bool_flags(),
        ["no-input", "yes", "confirm", "dry-run"]
    );
    assert_eq!(spec.required_flags(), ["reason"]);
    assert_eq!(spec.summary, "replace an item's value with a stated reason");
    assert_eq!(spec.tool, Some("correct"));
    assert_eq!(spec.section, Section::Write);
    assert_eq!(positionals(spec), [("ID", true), ("VALUE", true)]);
}

/// Every documented flag in this tranche carries a description and, when it
/// takes a value, a placeholder and non-empty kind.
#[test]
fn the_core_writes_tranche_passes_the_metadata_check() {
    let specs: Vec<&CommandSpec> = super::CORE_WRITES.iter().collect();
    assert_eq!(check_metadata(&specs), Ok(()));
}

/// `remember`/`admit` `--epistemic-kind` records the constant default
/// `cli_write_core` applies when the flag is omitted.
#[test]
fn epistemic_kind_metadata_default_is_fact() {
    for verb in ["remember", "admit"] {
        let kind = record(verb)
            .documented
            .iter()
            .find(|flag| flag.long == "epistemic-kind")
            .expect("the verb documents --epistemic-kind");
        assert_eq!(
            kind.default,
            Some("fact"),
            "{verb} --epistemic-kind default"
        );
    }
}

/// Every documented flag carries the kind its consumer parses: the enum
/// lists are the `clio_types` wire tables for categories and provenance,
/// `cli_write_core`'s two epistemic kinds, and `cli_write_mutate`'s two
/// rules; the unit-bounded confidences are `Float`, and every remaining
/// flag stays free text or the shared `--output` global.
#[test]
fn the_core_writes_flag_kinds_match_their_consumers() {
    for spec in super::CORE_WRITES {
        for flag in spec.documented {
            let Some(value) = flag.value else { continue };
            let expected = match (spec.name, flag.long) {
                ("remember" | "admit", "category") => {
                    ValueKind::Enum(clio_types::SemanticCategory::WIRE)
                }
                ("remember" | "admit", "epistemic-kind") => ValueKind::Enum(&["fact", "belief"]),
                ("remember" | "admit", "source-type") => {
                    ValueKind::Enum(clio_types::SourceType::WIRE)
                }
                ("update", "rule") => ValueKind::Enum(&["discrete", "continuous"]),
                ("remember" | "admit", "confidence") => ValueKind::Float,
                (_, "output") => ValueKind::Enum(&["json", "text"]),
                _ => ValueKind::Text,
            };
            assert_eq!(value.kind, expected, "`{} --{}`", spec.name, flag.long);
        }
    }
}
