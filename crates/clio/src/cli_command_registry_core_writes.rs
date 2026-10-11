// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the static records of the memory-write
//! commands: `remember`, `admit`, `summarize`, `consolidate`, `update`,
//! `invalidate`, `discard`, and `correct`.
//!
//! ## Owns
//! - The eight `CommandSpec` records and the `CORE_WRITES` tranche that lists
//!   them in whole-catalog order
//!
//! ## Does not own
//! - The record model or its rendering (see `cli_command_spec`)
//! - The registry and the shared global flags (see `cli_command_registry`)
//! - The command semantics (see `cli_write_core` and `cli_write_mutate`)
//!
//! ## Boundary
//!
//! Changes to one of these eight commands' flags or summary text should
//! normally require changes here. Changes to the write engine should not.
//!
//! Keep this module focused on the eight memory-write records.
//
// Each record carries the bytes its authored rows held: the summary is the
// catalog row, the usage line is the registered usage text, and the tool is the
// binding row, so both rendered surfaces keep their exact output.

use crate::cli_command_registry::{GLOBAL_ACTOR, GLOBAL_BANK, GLOBAL_OUTPUT};
use crate::cli_command_spec::{CommandSpec, Flag, Positional, Section};

/// These records print `--bank`/`--actor`, so only these ride along.
const IMPLICIT_GLOBALS_OUTSIDE_BANK_ACTOR: &[&str] = &["db", "backend", "no-input", "yes"];

/// The `consolidate` line prints `--bank` but not `--actor`, so `--actor` rides along.
const IMPLICIT_GLOBALS_OUTSIDE_BANK: &[&str] = &["db", "backend", "actor", "no-input", "yes"];

/// The `summarize` line prints neither `--bank` nor `--actor`, so every global rides along.
const IMPLICIT_GLOBALS: &[&str] = &["db", "backend", "bank", "actor", "no-input", "yes"];

/// The required `TEXT` positional of `remember` and `admit`.
const TEXT: Positional = Positional {
    label: "TEXT",
    required: true,
    description: "",
};

/// The required `ID` positional of `update`, `invalidate`, and `discard`.
const ID: Positional = Positional {
    label: "ID",
    required: true,
    description: "",
};

/// The required `VALUE` positional of `update` and `correct`.
const VALUE: Positional = Positional {
    label: "VALUE",
    required: true,
    description: "",
};

/// `remember` and `admit` cannot run without a category.
const CATEGORY_REQUIRED: Flag = Flag {
    required: true,
    ..Flag::value("category", "C")
};

/// `summarize` cannot run without its scope.
const SCOPE_REQUIRED: Flag = Flag {
    required: true,
    ..Flag::value("scope", "S")
};

/// `update` cannot run without its named rule.
const RULE_REQUIRED: Flag = Flag {
    required: true,
    ..Flag::value("rule", "discrete|continuous")
};

/// `discard` cannot run without its reason.
const REASON_REQUIRED: Flag = Flag {
    required: true,
    ..Flag::value("reason", "R")
};

/// The `--epistemic-kind fact|belief` choice of `remember` and `admit`.
const EPISTEMIC_KIND: Flag = Flag::value("epistemic-kind", "fact|belief");

/// The `--context` note `remember` and `admit` print under their usage lines.
const CONTEXT_NOTE: &str = "--context records where the memory came from, such as a conversation type or workflow stage. It is not searchable and recall does not match on it, so put anything you want to find later in TEXT; it is at most 4096 UTF-8 bytes and is rejected when empty or oversized, never truncated";

/// The `consolidate` note: what the maintenance wave does.
const CONSOLIDATE_NOTE: &str = "--bank selects the target bank for the maintenance wave; --force runs the full wave: refresh every dirty ancestor, then reconcile the bank's consolidated units";

/// The `admit` printed line: the parser's accepted set also carries the
/// `--file` batch form, which only the alternate line shows, so the line is
/// pinned.
const ADMIT_USAGE: &str = "clio admit TEXT --category C [--bank BANK] [--actor ACTOR] [--epistemic-kind fact|belief] [--confidence N] [--source-type T] [--evidence-ref E] [--context TEXT] [--output json|text]";

/// The `admit` alternate: the file spelling of the admission preview.
const ADMIT_FILE_ALTERNATE: &str = "clio admit --file ops.json [--bank BANK] [--actor ACTOR]";

/// The `--epistemic-kind`/`--source-type` pairing `remember`, `admit`,
/// `triple add`, `canonical put`, and `shared store` print under their usage
/// lines.
const SOURCE_TYPE_PAIRING_NOTE: &str = "--epistemic-kind and --source-type are a pair: --source-type (user_stated|agent_inferred|third_party) is required with belief and must be omitted with fact";

/// The `correct` printed line: `--confirm` gates the live run and prints, and
/// the line is pinned.
const CORRECT_USAGE: &str = "clio correct ID VALUE --reason R [--context TEXT] [--confirm] [--dry-run] [--bank BANK] [--actor ACTOR] [--output json|text]";

/// The `discard` printed line: `--confirm` gates the live run and prints bare,
/// but the confirm gate owns the refusal (a run without it is a dry run), so
/// the required set does not list it and the line is pinned.
const DISCARD_USAGE: &str = "clio discard ID --reason R --confirm [--dry-run] [--bank BANK] [--actor ACTOR] [--output json|text]";

/// `clio remember`: the flagship gated store, over the `store` tool.
pub(crate) const REMEMBER: CommandSpec = CommandSpec {
    name: "remember",
    aliases: &[],
    summary: "",
    notes: &[CONTEXT_NOTE, SOURCE_TYPE_PAIRING_NOTE],
    positionals: &[TEXT],
    documented: &[
        CATEGORY_REQUIRED,
        GLOBAL_BANK,
        GLOBAL_ACTOR,
        EPISTEMIC_KIND,
        Flag::value("confidence", "N"),
        Flag::value("source-type", "T"),
        Flag::value("evidence-ref", "E"),
        Flag::value("context", "TEXT"),
        Flag::boolean("allow-duplicate"),
        Flag::boolean("dry-run"),
        GLOBAL_OUTPUT,
    ],
    accepted_extra: IMPLICIT_GLOBALS_OUTSIDE_BANK_ACTOR,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[],
    tool: Some("store"),
    section: Section::Write,
};

/// `clio admit`: the read-only admission preview, over the `admit_preview`
/// tool; `--file` previews a whole batch and only the alternate line shows it.
pub(crate) const ADMIT: CommandSpec = CommandSpec {
    name: "admit",
    aliases: &[],
    summary: "score one write without storing it; `--file ops.json` previews a whole batch instead",
    notes: &[CONTEXT_NOTE, SOURCE_TYPE_PAIRING_NOTE],
    positionals: &[TEXT],
    documented: &[
        CATEGORY_REQUIRED,
        GLOBAL_BANK,
        GLOBAL_ACTOR,
        EPISTEMIC_KIND,
        Flag::value("confidence", "N"),
        Flag::value("source-type", "T"),
        Flag::value("evidence-ref", "E"),
        Flag::value("context", "TEXT"),
        Flag::value("file", "ops.json"),
        GLOBAL_OUTPUT,
    ],
    accepted_extra: IMPLICIT_GLOBALS_OUTSIDE_BANK_ACTOR,
    flag_aliases: &[],
    usage_override: Some(ADMIT_USAGE),
    usage_alternates: &[ADMIT_FILE_ALTERNATE],
    examples: &[],
    tool: Some("admit_preview"),
    section: Section::Read,
};

/// `clio summarize`: fold one scope into a consolidated unit.
pub(crate) const SUMMARIZE: CommandSpec = CommandSpec {
    name: "summarize",
    aliases: &[],
    summary: "summarize one scope into a consolidated unit",
    notes: &[],
    positionals: &[],
    documented: &[SCOPE_REQUIRED, Flag::boolean("dry-run"), GLOBAL_OUTPUT],
    accepted_extra: IMPLICIT_GLOBALS,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[],
    tool: Some("summarize"),
    section: Section::Write,
};

/// `clio consolidate`: the maintenance wave over one bank.
pub(crate) const CONSOLIDATE: CommandSpec = CommandSpec {
    name: "consolidate",
    aliases: &[],
    summary: "run the consolidation wave over a bank",
    notes: &[CONSOLIDATE_NOTE],
    positionals: &[],
    documented: &[
        GLOBAL_BANK,
        Flag::boolean("force"),
        Flag::boolean("dry-run"),
        GLOBAL_OUTPUT,
    ],
    accepted_extra: IMPLICIT_GLOBALS_OUTSIDE_BANK,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[],
    tool: Some("consolidate"),
    section: Section::Write,
};

/// `clio update`: set an item's value under a named rule.
pub(crate) const UPDATE: CommandSpec = CommandSpec {
    name: "update",
    aliases: &[],
    summary: "set an item's value under a named rule",
    notes: &[],
    positionals: &[ID, VALUE],
    documented: &[
        RULE_REQUIRED,
        Flag::boolean("dry-run"),
        GLOBAL_BANK,
        GLOBAL_ACTOR,
        GLOBAL_OUTPUT,
    ],
    accepted_extra: IMPLICIT_GLOBALS_OUTSIDE_BANK_ACTOR,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[],
    tool: Some("update"),
    section: Section::Write,
};

/// `clio invalidate`: close an item's validity; never deletes.
pub(crate) const INVALIDATE: CommandSpec = CommandSpec {
    name: "invalidate",
    aliases: &[],
    summary: "close an item's validity; never deletes",
    notes: &[],
    positionals: &[ID],
    documented: &[
        Flag::value("replacement", "ID"),
        Flag::boolean("dry-run"),
        GLOBAL_BANK,
        GLOBAL_ACTOR,
        GLOBAL_OUTPUT,
    ],
    accepted_extra: IMPLICIT_GLOBALS_OUTSIDE_BANK_ACTOR,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[],
    tool: Some("invalidate"),
    section: Section::Write,
};

/// `clio discard`: logged operational removal behind the confirm gate.
pub(crate) const DISCARD: CommandSpec = CommandSpec {
    name: "discard",
    aliases: &[],
    summary: "operational removal; distinct from compliance erasure",
    notes: &[],
    positionals: &[ID],
    documented: &[
        REASON_REQUIRED,
        Flag::boolean("confirm"),
        Flag::boolean("dry-run"),
        GLOBAL_BANK,
        GLOBAL_ACTOR,
        GLOBAL_OUTPUT,
    ],
    accepted_extra: IMPLICIT_GLOBALS_OUTSIDE_BANK_ACTOR,
    flag_aliases: &[],
    usage_override: Some(DISCARD_USAGE),
    usage_alternates: &[],
    examples: &[],
    tool: Some("discard"),
    section: Section::Write,
};

/// `clio correct`: an attributable value replacement that keeps history; `--confirm` gates the live run.
pub(crate) const CORRECT: CommandSpec = CommandSpec {
    name: "correct",
    aliases: &[],
    summary: "replace an item's value with a stated reason",
    notes: &[],
    positionals: &[ID, VALUE],
    documented: &[
        REASON_REQUIRED,
        Flag::value("context", "TEXT"),
        Flag::boolean("confirm"),
        Flag::boolean("dry-run"),
        GLOBAL_BANK,
        GLOBAL_ACTOR,
        GLOBAL_OUTPUT,
    ],
    accepted_extra: IMPLICIT_GLOBALS_OUTSIDE_BANK_ACTOR,
    flag_aliases: &[],
    usage_override: Some(CORRECT_USAGE),
    usage_alternates: &[],
    examples: &[],
    tool: Some("correct"),
    section: Section::Write,
};

/// The eight memory-write records, in whole-catalog order.
pub(crate) static CORE_WRITES: &[CommandSpec] = &[
    REMEMBER,
    ADMIT,
    SUMMARIZE,
    CONSOLIDATE,
    UPDATE,
    INVALIDATE,
    DISCARD,
    CORRECT,
];

#[cfg(test)]
#[path = "cli_command_registry_core_writes_tests.rs"]
mod tests;
