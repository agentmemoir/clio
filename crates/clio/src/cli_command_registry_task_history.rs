// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the static records of the commands headed by
//! `task`, `failure`, `memtree`, and `history`: `task get`, `task history`,
//! `task upsert`, `failure list`, `failure record`, `memtree query`,
//! `memtree get`, and `history temporal`.
//!
//! A read head can hold a write subcommand: `task upsert` and `failure record`
//! write, but they are classified under their read head like every other
//! record here.
//!
//! ## Owns
//! - The eight `CommandSpec` records and `TASK_HISTORY`, the tranche that
//!   lists them in head-group order
//!
//! ## Does not own
//! - The record model or its rendering (see `cli_command_spec`)
//! - The registry lookups and the global order of the rendered surfaces (see
//!   `cli_command_registry` and `cli_command_bindings`)
//! - The command semantics (see the history, task, and write surfaces)
//!
//! ## Boundary
//!
//! Changes to one of these commands' flags or summary text should normally
//! require changes here. Changes to the command implementations should not.
//!
//! Keep this module focused on the task, failure, memtree, and history records.

use crate::cli_command_registry::GLOBAL_OUTPUT;
use crate::cli_command_spec::{CommandSpec, Flag, Positional, Section};

/// The required `TASK_ID` positional of `task get`, `task history`,
/// `task upsert`, and `failure record`.
const TASK_ID: Positional = Positional {
    label: "TASK_ID",
    required: true,
    description: "",
};

/// The required `TASK_ID_OR_QUERY` positional of `failure list`.
const TASK_ID_OR_QUERY: Positional = Positional {
    label: "TASK_ID_OR_QUERY",
    required: true,
    description: "",
};

/// The required `NODE_ID` positional of `memtree get`.
const NODE_ID: Positional = Positional {
    label: "NODE_ID",
    required: true,
    description: "",
};

/// The required `TARGET` positional of `history temporal`.
const TARGET: Positional = Positional {
    label: "TARGET",
    required: true,
    description: "",
};

/// The `--explain` trace flag the read commands here document.
const EXPLAIN: Flag = Flag::boolean("explain");

/// The `--dry-run` preview flag the write commands here document.
const DRY_RUN: Flag = Flag::boolean("dry-run");

/// The required `--definition D` flag of `task upsert`.
const DEFINITION: Flag = Flag {
    required: true,
    ..Flag::value("definition", "D")
};

/// The required `--status S` flag of `task upsert`.
const STATUS: Flag = Flag {
    required: true,
    ..Flag::value("status", "S")
};

/// The required `--attempt-n N` flag of `failure record`.
const ATTEMPT_N: Flag = Flag {
    required: true,
    ..Flag::value("attempt-n", "N")
};

/// The required `--what-failed W` flag of `failure record`.
const WHAT_FAILED: Flag = Flag {
    required: true,
    ..Flag::value("what-failed", "W")
};

/// The required `--lesson L` flag of `failure record`.
const LESSON: Flag = Flag {
    required: true,
    ..Flag::value("lesson", "L")
};

/// The declared globals these commands accept without printing: every global
/// but `--output`, which each usage line documents.
const UNPRINTED_GLOBALS: &[&str] = &["db", "backend", "bank", "actor", "no-input", "yes"];

/// `clio task get`: one task and its current state, over the `task_get` tool.
pub(crate) const TASK_GET: CommandSpec = CommandSpec {
    name: "task get",
    aliases: &[],
    summary: "one task and its current state",
    notes: &[],
    positionals: &[TASK_ID],
    documented: &[EXPLAIN, GLOBAL_OUTPUT],
    accepted_extra: UNPRINTED_GLOBALS,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[],
    tool: Some("task_get"),
    section: Section::Read,
};

/// `clio task history`: a task's recorded transitions, over the
/// `task_history` tool.
pub(crate) const TASK_HISTORY_RECORD: CommandSpec = CommandSpec {
    name: "task history",
    aliases: &[],
    summary: "a task's recorded transitions",
    notes: &[],
    positionals: &[TASK_ID],
    documented: &[Flag::value("limit", "N"), EXPLAIN, GLOBAL_OUTPUT],
    accepted_extra: UNPRINTED_GLOBALS,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[],
    tool: Some("task_history"),
    section: Section::Read,
};

/// `clio task upsert`: create or update one task, over the `task_upsert` tool.
pub(crate) const TASK_UPSERT: CommandSpec = CommandSpec {
    name: "task upsert",
    aliases: &[],
    summary: "create or update one task",
    notes: &[],
    positionals: &[TASK_ID],
    documented: &[
        DEFINITION,
        STATUS,
        Flag::value("steps", "a,b"),
        Flag::value("evidence-ref", "E"),
        Flag::value("context", "TEXT"),
        DRY_RUN,
        GLOBAL_OUTPUT,
    ],
    accepted_extra: UNPRINTED_GLOBALS,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[],
    tool: Some("task_upsert"),
    section: Section::Write,
};

/// `clio failure list`: failures recorded against one task, over the
/// `failures_for_task` tool.
pub(crate) const FAILURE_LIST: CommandSpec = CommandSpec {
    name: "failure list",
    aliases: &[],
    summary: "failures recorded against one task",
    notes: &[],
    positionals: &[TASK_ID_OR_QUERY],
    documented: &[Flag::value("limit", "N"), EXPLAIN, GLOBAL_OUTPUT],
    accepted_extra: UNPRINTED_GLOBALS,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[],
    tool: Some("failures_for_task"),
    section: Section::Read,
};

/// `clio failure record`: one attempt, its failure, and the lesson, over the
/// `failure_record` tool.
pub(crate) const FAILURE_RECORD: CommandSpec = CommandSpec {
    name: "failure record",
    aliases: &[],
    summary: "record one attempt, its failure, and the lesson",
    notes: &[],
    positionals: &[TASK_ID],
    documented: &[
        ATTEMPT_N,
        WHAT_FAILED,
        LESSON,
        Flag::value("evidence-ref", "E"),
        Flag::value("context", "TEXT"),
        DRY_RUN,
        GLOBAL_OUTPUT,
    ],
    accepted_extra: UNPRINTED_GLOBALS,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[],
    tool: Some("failure_record"),
    section: Section::Write,
};

/// `clio memtree query`: walk the memory tree from a seed, over the
/// `memtree_query` tool.
pub(crate) const MEMTREE_QUERY: CommandSpec = CommandSpec {
    name: "memtree query",
    aliases: &[],
    summary: "walk the memory tree from a seed",
    notes: &[],
    positionals: &[],
    documented: &[
        Flag::value("time-start", "TS"),
        Flag::value("time-end", "TS"),
        Flag::value("task-id", "ID"),
        Flag::value("depth", "N"),
        Flag::value("limit", "N"),
        EXPLAIN,
        GLOBAL_OUTPUT,
    ],
    accepted_extra: UNPRINTED_GLOBALS,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[],
    tool: Some("memtree_query"),
    section: Section::Read,
};

/// `clio memtree get`: one memory-tree node and its lineage, over the
/// `memtree_get` tool.
pub(crate) const MEMTREE_GET: CommandSpec = CommandSpec {
    name: "memtree get",
    aliases: &[],
    summary: "one memory-tree node and its lineage",
    notes: &[],
    positionals: &[NODE_ID],
    documented: &[EXPLAIN, GLOBAL_OUTPUT],
    accepted_extra: UNPRINTED_GLOBALS,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[],
    tool: Some("memtree_get"),
    section: Section::Read,
};

/// `clio history temporal`: the item history as it stood at a point in time,
/// over the `temporal_history` tool.
pub(crate) const HISTORY_TEMPORAL: CommandSpec = CommandSpec {
    name: "history temporal",
    aliases: &[],
    summary: "the item history as it stood at a point in time",
    notes: &[],
    positionals: &[TARGET],
    documented: &[
        Flag::value("as-of", "TS"),
        Flag::value("time-axis", "valid|transaction"),
        EXPLAIN,
        GLOBAL_OUTPUT,
    ],
    accepted_extra: UNPRINTED_GLOBALS,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[],
    tool: Some("temporal_history"),
    section: Section::Read,
};

/// The eight records, in head-group order: `task`, `failure`, `memtree`, then
/// `history`.
pub(crate) static TASK_HISTORY: &[CommandSpec] = &[
    TASK_GET,
    TASK_HISTORY_RECORD,
    TASK_UPSERT,
    FAILURE_LIST,
    FAILURE_RECORD,
    MEMTREE_QUERY,
    MEMTREE_GET,
    HISTORY_TEMPORAL,
];

#[cfg(test)]
#[path = "cli_command_registry_task_history_tests.rs"]
mod tests;
