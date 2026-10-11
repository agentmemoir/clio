// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the static records of the five `ranking` and
//! `retention` commands: `ranking get`, `ranking set`, `retention get`,
//! `retention set`, and `retention schema`.
//!
//! ## Owns
//! - The five `CommandSpec` records and the tranche that lists them in capture
//!   order, each carrying the bytes its authored usage row and catalog row held
//! - The `ranking` group's verb keys, taken from the records' own names so the
//!   dispatcher has no second list to drift
//! - The declared globals each record accepts without printing: the retention
//!   surface's shared parser admits the globals by its own rule, so its
//!   records name the ones their usage lines do not print
//!
//! ## Does not own
//! - The record model or its rendering (see `cli_command_spec`)
//! - The registry and the shared global flags (see `cli_command_registry`)
//! - Ranking semantics (see `ranking_cli`) or retention semantics (see
//!   `retention_cli`)
//!
//! ## Boundary
//!
//! Changes to one of these five commands' flags, usage text, or summary should
//! normally require changes here. Changes to ranking or retention semantics
//! should not.
//!
//! Keep this module focused on the five ranking and retention records.
//
// Each record carries the bytes its authored rows held: the summary is the
// catalog row, the usage line is the registered usage text, so both rendered
// surfaces and the frozen golden capture keep their exact output.

use crate::cli_command_registry::GLOBAL_OUTPUT;
use crate::cli_command_spec::{CommandSpec, Flag, Section};

/// The declared globals a ranking leaf accepts without printing: the shared
/// parser admits every declared global, so the leaves that print only their
/// own flags and `--output` list the other six here.
const UNPRINTED: &[&str] = &["db", "backend", "bank", "actor", "no-input", "yes"];

/// The declared globals a retention leaf that documents `--bank` still accepts
/// without printing.
const RETENTION_UNPRINTED: &[&str] = &["db", "backend", "actor", "output", "no-input", "yes"];

/// The declared globals `retention schema` accepts without printing: its line
/// names no flag.
const SCHEMA_UNPRINTED: &[&str] = &[
    "db", "backend", "bank", "actor", "output", "no-input", "yes",
];

/// The required `--patch JSON` of `ranking set`: the usage line writes it bare
/// because the verb refuses to run without one.
const PATCH: Flag = Flag {
    required: true,
    ..Flag::text(
        "patch",
        "JSON",
        "Carries the JSON weight patch that `ranking_cli::set_call` parses and passes to the `ranking_env_set` tool.",
    )
};

/// The required `--profile JSON` of `retention set`: the usage line writes it
/// bare because the verb refuses to run without one.
const PROFILE: Flag = Flag {
    required: true,
    ..Flag::text(
        "profile",
        "JSON",
        "Carries the JSON profile that `retention_cli::profile_from_flags` parses into a `RetentionProfile`.",
    )
};

/// The `--bank BANK` of `retention get` and `retention set`: `bank_of` falls
/// back to `default` when the flag is absent.
const RETENTION_BANK: Flag = Flag {
    default: Some("default"),
    ..Flag::text(
        "bank",
        "BANK",
        "Scopes the command to one bank; `retention_cli::bank_of` falls back to `default` when it is absent.",
    )
};

/// The `--scope bank|deployment` of `retention set`: the verb stores per bank
/// unless the caller asks for the deployment default.
const RETENTION_SCOPE: Flag = Flag {
    default: Some("bank"),
    ..Flag::choice(
        "scope",
        "bank|deployment",
        "Selects whether the profile is stored per bank or as the deployment default; `retention_cli::run` accepts `bank` or `deployment` and falls back to `bank`.",
        &["bank", "deployment"],
    )
};

/// `clio ranking get`: the effective ranking/admission environment.
pub(crate) const RANKING_GET: CommandSpec = CommandSpec {
    name: "ranking get",
    aliases: &[],
    summary: "read the ranking environment",
    notes: &[],
    positionals: &[],
    documented: &[GLOBAL_OUTPUT],
    accepted_extra: UNPRINTED,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[],
    tool: Some("ranking_env_get"),
    section: Section::Read,
};

/// `clio ranking set`: apply a validated weight patch.
pub(crate) const RANKING_SET: CommandSpec = CommandSpec {
    name: "ranking set",
    aliases: &[],
    summary: "patch the ranking environment",
    notes: &[],
    positionals: &[],
    documented: &[
        PATCH,
        Flag::text(
            "patch-file",
            "FILE",
            "Names the file that `ranking_cli::set_call` reads the JSON weight patch from when `--patch` is absent.",
        ),
        Flag::toggle(
            "dry-run",
            "Returns the normalized weights without applying them; `ranking_cli::set_call` sets the `ranking_env_set` tool's `dry_run` argument.",
        ),
        GLOBAL_OUTPUT,
    ],
    accepted_extra: UNPRINTED,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[],
    tool: Some("ranking_env_set"),
    section: Section::Write,
};

/// `clio retention get`: the effective per-bank profile.
pub(crate) const RETENTION_GET: CommandSpec = CommandSpec {
    name: "retention get",
    aliases: &[],
    summary: "Show the effective per-bank retention profile",
    notes: &[],
    positionals: &[],
    documented: &[
        RETENTION_BANK,
        Flag::text(
            "profiles",
            "FILE",
            "Names the retention store file that `retention_cli::open_runtime` opens; `CLIO_RETENTION_PROFILES` decides it when absent.",
        ),
    ],
    accepted_extra: RETENTION_UNPRINTED,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[],
    tool: None,
    section: Section::Read,
};

/// `clio retention set`: store a validated profile.
pub(crate) const RETENTION_SET: CommandSpec = CommandSpec {
    name: "retention set",
    aliases: &[],
    summary: "Store a validated retention profile (persists to --profiles/env)",
    notes: &[],
    positionals: &[],
    documented: &[
        RETENTION_BANK,
        RETENTION_SCOPE,
        PROFILE,
        Flag::text(
            "profile-file",
            "FILE",
            "Names the file that `retention_cli::profile_from_flags` reads the JSON profile from when `--profile` is absent.",
        ),
        Flag::text(
            "profiles",
            "FILE",
            "Names the retention store file that `retention_cli::open_runtime` opens; `CLIO_RETENTION_PROFILES` decides it when absent.",
        ),
    ],
    accepted_extra: RETENTION_UNPRINTED,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[],
    tool: None,
    section: Section::Write,
};

/// `clio retention schema`: the profile's JSON Schema, takes no flags.
pub(crate) const RETENTION_SCHEMA: CommandSpec = CommandSpec {
    name: "retention schema",
    aliases: &[],
    summary: "Print the retention profile JSON Schema",
    notes: &[],
    positionals: &[],
    documented: &[],
    accepted_extra: SCHEMA_UNPRINTED,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[],
    tool: None,
    section: Section::Read,
};

/// The `ranking` group's verb keys: the two ranking records' own names, in
/// tranche order. `RuntimeGroup::verbs` needs a static slice, so the names are
/// taken from the records rather than retyped; the tranche tests pin this list
/// equal to the prefix-derived registry list.
pub(crate) static RANKING_VERBS: &[&str] = &[RANKING_GET.name, RANKING_SET.name];

/// The five ranking and retention records, in capture order.
pub(crate) static RANKING_RETENTION_LEAVES: &[CommandSpec] = &[
    RANKING_GET,
    RANKING_SET,
    RETENTION_GET,
    RETENTION_SET,
    RETENTION_SCHEMA,
];

#[cfg(test)]
#[path = "cli_command_registry_ranking_retention_tests.rs"]
mod tests;
