// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the static records of the workspace write
//! commands: `scratchpad write`, `scratchpad read`, `scratchpad clear`,
//! `canonical put`, and `canonical get`.
//!
//! It is the sibling the `cli_command_registry_persona_local` module declares
//! and re-exports, so the tranche's `PERSONA_LOCAL` list can name these records
//! while each half of the tranche stays within the size cap.
//!
//! ## Owns
//! - The five `CommandSpec` records and the positionals and flags only they use
//! - The description, value kind, and default of each documented flag
//!
//! ## Does not own
//! - The persona and maintenance records and the tranche list (see
//!   `cli_command_registry_persona_local`)
//! - The record model or its rendering (see `cli_command_spec`)
//! - The command semantics (see the scratchpad and canonical write surface)
//!
//! ## Boundary
//!
//! Changes to one of these commands' flags or summary text should normally
//! require changes here. Changes to the command implementations should not.
//!
//! Keep this module focused on the scratchpad and canonical records.

use super::{CONFIDENCE, DRY_RUN, EVIDENCE_REF};
use crate::cli_command_registry::GLOBAL_OUTPUT;
use crate::cli_command_spec::{CommandSpec, Example, Flag, Positional, Section};
use clio_types::{SemanticCategory, SourceType};

/// The required `KEY` positional of `scratchpad write`.
const KEY: Positional = Positional {
    label: "KEY",
    required: true,
    description: "The scratchpad key to store the content under.",
};

/// The optional `KEY` positional of `scratchpad read` and `scratchpad clear`.
const OPTIONAL_KEY: Positional = Positional {
    label: "KEY",
    required: false,
    description: "The scratchpad key to read or drop; omit it to use the whole session pad.",
};

/// The required `CONTENT` positional of `scratchpad write`.
const CONTENT: Positional = Positional {
    label: "CONTENT",
    required: true,
    description: "The content stored under the key for this session.",
};

/// The required `KEY` positional of `canonical put` and `canonical get`.
const CANONICAL_KEY: Positional = Positional {
    label: "KEY",
    required: true,
    description: "The canonical slot key to read or upsert.",
};

/// The required `VALUE` positional of `canonical put`.
const CANONICAL_VALUE: Positional = Positional {
    label: "VALUE",
    required: true,
    description: "The value stored in the canonical slot.",
};

/// The `--session S` of the scratchpad commands.
const SESSION: Flag = Flag::text("session", "S", "`insert_session` sets the session id.");

/// The scratchpad lines print the global `--bank BANK` placeholder.
const SCRATCHPAD_BANK: Flag =
    Flag::text("bank", "BANK", "`base_args` scopes the call to one bank.");

/// The scratchpad lines print the global `--actor ACTOR` placeholder.
const SCRATCHPAD_ACTOR: Flag =
    Flag::text("actor", "ACTOR", "`base_args` names the acting identity.");

/// `canonical put` cannot run without its selector flags.
const CATEGORY_REQUIRED: Flag = Flag {
    required: true,
    ..Flag::choice(
        "category",
        "C",
        "`canonical_put_call` requires the slot category from `task_spec|schema|tool_config|output_constraint|persona`.",
        SemanticCategory::WIRE,
    )
};

/// The second `canonical put` selector.
const EPISTEMIC_KIND_REQUIRED: Flag = Flag {
    required: true,
    ..Flag::choice(
        "epistemic-kind",
        "K",
        "`canonical_put_call` requires and validates `fact` or `belief`.",
        &["fact", "belief"],
    )
};

/// The `--source-type T` of `canonical put`.
const SOURCE_TYPE: Flag = Flag::choice(
    "source-type",
    "T",
    "`source_type_arg` requires it with `belief`: `user_stated|agent_inferred|third_party`.",
    SourceType::WIRE,
);

/// The `--source-text TEXT` of `canonical put`.
const SOURCE_TEXT: Flag = Flag::text(
    "source-text",
    "TEXT",
    "`canonical_put_call` sets the `source_text`.",
);

/// The `--context C` of `canonical put`.
const CANONICAL_CONTEXT: Flag = Flag::text(
    "context",
    "C",
    "`context_arg` bounds the context to 4096 bytes.",
);

/// The `--as-of TS` of `canonical get`.
const AS_OF: Flag = Flag::text(
    "as-of",
    "TS",
    "`canonical_get_call` sets the point-in-time `as_of`.",
);

/// The `--epistemic-kind`/`--source-type` pairing `canonical put` prints under
/// its usage line.
const SOURCE_TYPE_PAIRING_NOTE: &str = "--epistemic-kind and --source-type are a pair: --source-type (user_stated|agent_inferred|third_party) is required with belief and must be omitted with fact";

/// The global flags the canonical records accept without printing.
const IMPLICIT_GLOBALS: &[&str] = &["db", "backend", "bank", "actor", "no-input", "yes"];

/// The scratchpad records print `--bank`/`--actor`, so only these ride along.
const IMPLICIT_GLOBALS_OUTSIDE_BANK_ACTOR: &[&str] = &["db", "backend", "no-input", "yes"];

/// `clio scratchpad write`: store one ephemeral session key.
pub(crate) const SCRATCHPAD_WRITE: CommandSpec = CommandSpec {
    name: "scratchpad write",
    aliases: &[],
    summary: "store an ephemeral key for this session",
    notes: &[],
    positionals: &[KEY, CONTENT],
    documented: &[SESSION, SCRATCHPAD_BANK, SCRATCHPAD_ACTOR, GLOBAL_OUTPUT],
    accepted_extra: IMPLICIT_GLOBALS_OUTSIDE_BANK_ACTOR,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[
        Example {
            command: "clio scratchpad write draft \"deploy notes\" --db ./memory.db",
            comment: None,
        },
        Example {
            command: "clio scratchpad write draft \"deploy notes\" --session s1",
            comment: None,
        },
        Example {
            command: "clio scratchpad write draft \"deploy notes\" --session s1 -o json",
            comment: None,
        },
    ],
    tool: Some("scratchpad_write"),
    section: Section::Write,
};

/// `clio scratchpad read`: one key, or the whole session pad.
pub(crate) const SCRATCHPAD_READ: CommandSpec = CommandSpec {
    name: "scratchpad read",
    aliases: &[],
    summary: "read one key, or the whole pad",
    notes: &[],
    positionals: &[OPTIONAL_KEY],
    documented: &[SESSION, SCRATCHPAD_BANK, SCRATCHPAD_ACTOR, GLOBAL_OUTPUT],
    accepted_extra: IMPLICIT_GLOBALS_OUTSIDE_BANK_ACTOR,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[
        Example {
            command: "clio scratchpad read --session s1 --db ./memory.db",
            comment: Some("whole pad"),
        },
        Example {
            command: "clio scratchpad read draft --session s1",
            comment: None,
        },
        Example {
            command: "clio scratchpad read draft -o json",
            comment: None,
        },
    ],
    tool: Some("scratchpad_read"),
    section: Section::Read,
};

/// `clio scratchpad clear`: drop one key, or the whole session pad.
pub(crate) const SCRATCHPAD_CLEAR: CommandSpec = CommandSpec {
    name: "scratchpad clear",
    aliases: &[],
    summary: "drop one key, or the whole pad",
    notes: &[],
    positionals: &[OPTIONAL_KEY],
    documented: &[SESSION, SCRATCHPAD_BANK, SCRATCHPAD_ACTOR, GLOBAL_OUTPUT],
    accepted_extra: IMPLICIT_GLOBALS_OUTSIDE_BANK_ACTOR,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[
        Example {
            command: "clio scratchpad clear --session s1 --db ./memory.db",
            comment: Some("whole pad"),
        },
        Example {
            command: "clio scratchpad clear draft --session s1",
            comment: None,
        },
    ],
    tool: Some("scratchpad_clear"),
    section: Section::Write,
};

/// `clio canonical put`: gated single-slot upsert over `canonical_put`.
pub(crate) const CANONICAL_PUT: CommandSpec = CommandSpec {
    name: "canonical put",
    aliases: &[],
    summary: "gated single-slot upsert with bi-temporal supersession",
    notes: &[SOURCE_TYPE_PAIRING_NOTE],
    positionals: &[CANONICAL_KEY, CANONICAL_VALUE],
    documented: &[
        CATEGORY_REQUIRED,
        EPISTEMIC_KIND_REQUIRED,
        SOURCE_TYPE,
        CONFIDENCE,
        SOURCE_TEXT,
        EVIDENCE_REF,
        CANONICAL_CONTEXT,
        DRY_RUN,
        GLOBAL_OUTPUT,
    ],
    accepted_extra: IMPLICIT_GLOBALS,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[
        Example {
            command: "clio canonical put style concise --category persona --epistemic-kind fact --db ./memory.db",
            comment: None,
        },
        Example {
            command: "clio canonical put style concise --category persona --epistemic-kind fact --dry-run",
            comment: Some("preview"),
        },
        Example {
            command: "clio canonical put style concise --category persona --epistemic-kind belief --source-type agent_inferred -o json",
            comment: None,
        },
    ],
    tool: Some("canonical_put"),
    section: Section::Write,
};

/// `clio canonical get`: the active or point-in-time canonical slot.
pub(crate) const CANONICAL_GET: CommandSpec = CommandSpec {
    name: "canonical get",
    aliases: &[],
    summary: "read one canonical key",
    notes: &[],
    positionals: &[CANONICAL_KEY],
    documented: &[AS_OF, GLOBAL_OUTPUT],
    accepted_extra: IMPLICIT_GLOBALS,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[
        Example {
            command: "clio canonical get style --db ./memory.db",
            comment: None,
        },
        Example {
            command: "clio canonical get style --as-of 2026-01-01",
            comment: None,
        },
        Example {
            command: "clio canonical get style -o json",
            comment: None,
        },
    ],
    tool: Some("canonical_get"),
    section: Section::Read,
};

#[cfg(test)]
#[path = "cli_command_registry_persona_local_workspace_tests.rs"]
mod tests;
