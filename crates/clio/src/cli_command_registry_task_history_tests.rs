// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the task, failure, memory-tree, and
//! temporal-history records against today's usage table and parser flag sets.
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

use crate::cli_command_registry_test_support::{assert_example_count, positionals, record};
use crate::cli_command_spec::{CommandSpec, Section};

/// The tranche's record names, in head-group order.
const NAMES: [&str; 8] = [
    "task get",
    "task history",
    "task upsert",
    "failure list",
    "failure record",
    "memtree query",
    "memtree get",
    "history temporal",
];

/// `task get` renders today's usage text and builds today's parser sets.
#[test]
fn task_get_matches_the_registered_usage_and_parser() {
    let spec = record("task get");
    assert_eq!(
        spec.full_usage(),
        "usage: clio task get TASK_ID [--explain] [--bank BANK] [--output json|text]",
        "task get full usage"
    );
    assert_eq!(
        spec.usage_line(),
        "clio task get TASK_ID [--explain] [--bank BANK] [--output json|text]",
        "task get usage line"
    );
    assert_eq!(
        spec.parser_spec().value_flags(),
        ["db", "backend", "bank", "actor", "output"]
    );
    assert_eq!(
        spec.parser_spec().bool_flags(),
        ["no-input", "yes", "explain"]
    );
    assert_eq!(spec.summary, "one task and its current state");
    assert_eq!(spec.tool, Some("task_get"));
    assert_eq!(spec.section, Section::Read);
    assert_eq!(positionals(spec), [("TASK_ID", true)]);
    assert!(spec.required_flags().is_empty(), "task get required flags");
}

/// `task history` renders today's usage text and builds today's parser sets.
#[test]
fn task_history_matches_the_registered_usage_and_parser() {
    let spec = record("task history");
    assert_eq!(
        spec.full_usage(),
        "usage: clio task history TASK_ID [--limit N] [--explain] [--bank BANK] [--output json|text]",
        "task history full usage"
    );
    assert_eq!(
        spec.usage_line(),
        "clio task history TASK_ID [--limit N] [--explain] [--bank BANK] [--output json|text]",
        "task history usage line"
    );
    assert_eq!(
        spec.parser_spec().value_flags(),
        ["db", "backend", "bank", "actor", "output", "limit"]
    );
    assert_eq!(
        spec.parser_spec().bool_flags(),
        ["no-input", "yes", "explain"]
    );
    assert_eq!(spec.summary, "a task's recorded transitions");
    assert_eq!(spec.tool, Some("task_history"));
    assert_eq!(spec.section, Section::Read);
    assert_eq!(positionals(spec), [("TASK_ID", true)]);
    assert!(
        spec.required_flags().is_empty(),
        "task history required flags"
    );
}

/// `task upsert` renders today's usage text and builds today's parser sets,
/// including the two required bits its refusal message is built from.
#[test]
fn task_upsert_matches_the_registered_usage_and_parser() {
    let spec = record("task upsert");
    assert_eq!(
        spec.full_usage(),
        "usage: clio task upsert TASK_ID --definition D --status S [--steps a,b] [--evidence-ref E] [--context TEXT] [--dry-run] [--bank BANK] [--output json|text]",
        "task upsert full usage"
    );
    assert_eq!(
        spec.usage_line(),
        "clio task upsert TASK_ID --definition D --status S [--steps a,b] [--evidence-ref E] [--context TEXT] [--dry-run] [--bank BANK] [--output json|text]",
        "task upsert usage line"
    );
    assert_eq!(
        spec.parser_spec().value_flags(),
        [
            "db",
            "backend",
            "bank",
            "actor",
            "output",
            "definition",
            "status",
            "steps",
            "evidence-ref",
            "context"
        ]
    );
    assert_eq!(
        spec.parser_spec().bool_flags(),
        ["no-input", "yes", "dry-run"]
    );
    assert_eq!(spec.summary, "create or update one task");
    assert_eq!(spec.tool, Some("task_upsert"));
    assert_eq!(spec.section, Section::Write);
    assert_eq!(positionals(spec), [("TASK_ID", true)]);
    assert_eq!(spec.required_flags(), ["definition", "status"]);
}

/// `failure list` renders today's usage text and builds today's parser sets.
#[test]
fn failure_list_matches_the_registered_usage_and_parser() {
    let spec = record("failure list");
    assert_eq!(
        spec.full_usage(),
        "usage: clio failure list TASK_ID [--limit N] [--explain] [--bank BANK] [--output json|text]",
        "failure list full usage"
    );
    assert_eq!(
        spec.usage_line(),
        "clio failure list TASK_ID [--limit N] [--explain] [--bank BANK] [--output json|text]",
        "failure list usage line"
    );
    assert_eq!(
        spec.parser_spec().value_flags(),
        ["db", "backend", "bank", "actor", "output", "limit"]
    );
    assert_eq!(
        spec.parser_spec().bool_flags(),
        ["no-input", "yes", "explain"]
    );
    assert_eq!(spec.summary, "failures recorded against one task");
    assert_eq!(spec.tool, Some("failures_for_task"));
    assert_eq!(spec.section, Section::Read);
    assert_eq!(positionals(spec), [("TASK_ID", true)]);
    assert!(
        spec.required_flags().is_empty(),
        "failure list required flags"
    );
}

/// `failure record` renders today's usage text and builds today's parser sets,
/// including the three required bits its refusal message is built from.
#[test]
fn failure_record_matches_the_registered_usage_and_parser() {
    let spec = record("failure record");
    assert_eq!(
        spec.full_usage(),
        "usage: clio failure record TASK_ID --attempt-n N --what-failed W --lesson L [--evidence-ref E] [--context TEXT] [--dry-run] [--bank BANK] [--output json|text]",
        "failure record full usage"
    );
    assert_eq!(
        spec.usage_line(),
        "clio failure record TASK_ID --attempt-n N --what-failed W --lesson L [--evidence-ref E] [--context TEXT] [--dry-run] [--bank BANK] [--output json|text]",
        "failure record usage line"
    );
    assert_eq!(
        spec.parser_spec().value_flags(),
        [
            "db",
            "backend",
            "bank",
            "actor",
            "output",
            "attempt-n",
            "what-failed",
            "lesson",
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
        "record one attempt, its failure, and the lesson"
    );
    assert_eq!(spec.tool, Some("failure_record"));
    assert_eq!(spec.section, Section::Write);
    assert_eq!(positionals(spec), [("TASK_ID", true)]);
    assert_eq!(
        spec.required_flags(),
        ["attempt-n", "what-failed", "lesson"]
    );
}

/// `memtree query` renders today's usage text and builds today's parser sets.
#[test]
fn memtree_query_matches_the_registered_usage_and_parser() {
    let spec = record("memtree query");
    assert_eq!(
        spec.full_usage(),
        "usage: clio memtree query [--time-start TS] [--time-end TS] [--task-id ID] [--depth N] [--limit N] [--explain] [--bank BANK] [--output json|text]",
        "memtree query full usage"
    );
    assert_eq!(
        spec.usage_line(),
        "clio memtree query [--time-start TS] [--time-end TS] [--task-id ID] [--depth N] [--limit N] [--explain] [--bank BANK] [--output json|text]",
        "memtree query usage line"
    );
    assert_eq!(
        spec.parser_spec().value_flags(),
        [
            "db",
            "backend",
            "bank",
            "actor",
            "output",
            "time-start",
            "time-end",
            "task-id",
            "depth",
            "limit"
        ]
    );
    assert_eq!(
        spec.parser_spec().bool_flags(),
        ["no-input", "yes", "explain"]
    );
    assert_eq!(spec.summary, "walk the memory tree from a seed");
    assert_eq!(spec.tool, Some("memtree_query"));
    assert_eq!(spec.section, Section::Read);
    assert!(spec.positionals.is_empty(), "{:?}", spec.positionals);
    assert!(
        spec.required_flags().is_empty(),
        "memtree query required flags"
    );
}

/// `memtree get` renders today's usage text and builds today's parser sets.
#[test]
fn memtree_get_matches_the_registered_usage_and_parser() {
    let spec = record("memtree get");
    assert_eq!(
        spec.full_usage(),
        "usage: clio memtree get NODE_ID [--explain] [--bank BANK] [--output json|text]",
        "memtree get full usage"
    );
    assert_eq!(
        spec.usage_line(),
        "clio memtree get NODE_ID [--explain] [--bank BANK] [--output json|text]",
        "memtree get usage line"
    );
    assert_eq!(
        spec.parser_spec().value_flags(),
        ["db", "backend", "bank", "actor", "output"]
    );
    assert_eq!(
        spec.parser_spec().bool_flags(),
        ["no-input", "yes", "explain"]
    );
    assert_eq!(spec.summary, "one memory-tree node and its lineage");
    assert_eq!(spec.tool, Some("memtree_get"));
    assert_eq!(spec.section, Section::Read);
    assert_eq!(positionals(spec), [("NODE_ID", true)]);
    assert!(
        spec.required_flags().is_empty(),
        "memtree get required flags"
    );
}

/// `history temporal` renders today's usage text and builds today's parser
/// sets.
#[test]
fn history_temporal_matches_the_registered_usage_and_parser() {
    let spec = record("history temporal");
    assert_eq!(
        spec.full_usage(),
        "usage: clio history temporal TARGET [--as-of TS] [--time-axis valid|transaction] [--explain] [--bank BANK] [--output json|text]",
        "history temporal full usage"
    );
    assert_eq!(
        spec.usage_line(),
        "clio history temporal TARGET [--as-of TS] [--time-axis valid|transaction] [--explain] [--bank BANK] [--output json|text]",
        "history temporal usage line"
    );
    assert_eq!(
        spec.parser_spec().value_flags(),
        [
            "db",
            "backend",
            "bank",
            "actor",
            "output",
            "as-of",
            "time-axis"
        ]
    );
    assert_eq!(
        spec.parser_spec().bool_flags(),
        ["no-input", "yes", "explain"]
    );
    assert_eq!(
        spec.summary,
        "the item history as it stood at a point in time"
    );
    assert_eq!(spec.tool, Some("temporal_history"));
    assert_eq!(spec.section, Section::Read);
    assert_eq!(positionals(spec), [("TARGET", true)]);
    assert!(
        spec.required_flags().is_empty(),
        "history temporal required flags"
    );
}

/// The tranche lists these eight records once each, in head-group order.
#[test]
fn the_task_history_tranche_lists_its_eight_records() {
    let listed: Vec<&str> = super::TASK_HISTORY.iter().map(|spec| spec.name).collect();
    assert_eq!(listed, NAMES);
}

/// The tranche's records pass `validate` against the catalog inventory.
#[test]
fn the_task_history_tranche_validates_cleanly() {
    let inventory: Vec<&str> = crate::command_catalog::catalog_commands().collect();
    let records: Vec<&CommandSpec> = super::TASK_HISTORY.iter().collect();
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
        crate::cli_read_args::required_flags("task upsert"),
        ["definition", "status"]
    );
    assert_eq!(
        crate::cli_read_args::required_flags("failure record"),
        ["attempt-n", "what-failed", "lesson"]
    );
}

/// These records carry no aliases, flag aliases, overrides, alternates, or
/// prose; each carries two to four examples. This pins that none of them grew
/// quietly.
#[test]
fn the_eight_records_carry_no_extra_surfaces() {
    for name in NAMES {
        let spec = record(name);
        assert!(spec.aliases.is_empty(), "{name} aliases");
        assert!(spec.flag_aliases.is_empty(), "{name} flag aliases");
        assert!(spec.usage_override.is_none(), "{name} override");
        assert!(spec.usage_alternates.is_empty(), "{name} alternates");
        assert_example_count(name, spec);
        assert!(spec.notes.is_empty(), "{name} notes");
    }
}
