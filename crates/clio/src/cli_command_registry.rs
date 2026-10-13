// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the command registry: the shared global flags
//! and the ordered lookup over every command tranche.
//!
//! ## Owns
//! - `GLOBAL_VALUE_FLAGS` and `GLOBAL_BOOL_FLAGS` and their name lookups
//! - `TRANCHES`, the tranche order, and `all` / `find` / `resolve` /
//!   `resolve_in` / `names` / `aliases`
//!
//! ## Does not own
//! - The command records themselves (see the `cli_command_registry_*` record
//!   modules)
//! - The static-data contract over one set of records (see
//!   `cli_command_registry_contract`, re-exported here so callers keep one
//!   import path)
//! - The record model or its rendering (see `cli_command_spec`)
//!
//! ## Boundary
//!
//! Changes to the global flag set or the tranche order should normally require
//! changes here. Changes to one command's flags should not.
//!
//! Keep this module focused on the registry and the global flags.
//

use crate::cli_command_registry_auth_config as auth_config;
use crate::cli_command_registry_backup as backup_leaves;
use crate::cli_command_registry_compliance as compliance_leaves;
use crate::cli_command_registry_core_reads as core_reads;
use crate::cli_command_registry_core_writes as core_writes;
use crate::cli_command_registry_graph_records as graph_records;
use crate::cli_command_registry_persona_local as persona_local;
use crate::cli_command_registry_ranking_retention as ranking_retention;
use crate::cli_command_registry_shared_workspace as shared_workspace;
use crate::cli_command_registry_surface as surface;
use crate::cli_command_registry_sync as sync_leaves;
use crate::cli_command_registry_task_history as task_history;
use crate::cli_command_registry_write_operations as write_operations;
use crate::cli_command_spec::{CommandSpec, Flag};

/// The `--db` path global.
pub(crate) const GLOBAL_DB: Flag = Flag::text(
    "db",
    "PATH",
    "Overrides the store address that `db_target::resolve_target` opens; config and environment decide it when absent.",
);
/// The `--backend` global.
pub(crate) const GLOBAL_BACKEND: Flag = Flag::choice(
    "backend",
    "sqlite|postgres",
    "Selects the store backend that `db_target::resolve_target` hands to the resolver; config or the address decides it when absent.",
    &["sqlite", "postgres"],
);
/// The `--bank` global.
pub(crate) const GLOBAL_BANK: Flag = Flag::text(
    "bank",
    "BANK",
    "Scopes the command to one bank; `cli_read_args::base_args` passes it to the tool and the write scope defaults to `default`.",
);
/// The `--actor` global.
pub(crate) const GLOBAL_ACTOR: Flag = Flag::text(
    "actor",
    "ACTOR",
    "Names the acting identity; `cli_read_args::base_args` passes it to the tool and `auth_cli` reads it for the actor commands.",
);
/// The `--output` global; `-o` is its short spelling.
pub(crate) const GLOBAL_OUTPUT: Flag = Flag {
    short: Some('o'),
    ..Flag::choice(
        "output",
        "json|text",
        "Selects the output mode that `cli_output::resolve_output` returns; the terminal decides it when absent.",
        &["json", "text"],
    )
};
/// The `--no-input` global.
pub(crate) const GLOBAL_NO_INPUT: Flag = Flag::toggle(
    "no-input",
    "Refuses interactive input; `backup_restore_cli` treats a run with it as non-interactive.",
);
/// The `--yes` global.
pub(crate) const GLOBAL_YES: Flag = Flag::toggle(
    "yes",
    "Answers yes to the destructive-confirmation gate that `cli_confirm::confirmed` applies.",
);

/// Value-taking global flags, in the order every parser spec merges them.
pub(crate) static GLOBAL_VALUE_FLAGS: &[Flag] = &[
    GLOBAL_DB,
    GLOBAL_BACKEND,
    GLOBAL_BANK,
    GLOBAL_ACTOR,
    GLOBAL_OUTPUT,
];

/// Boolean global flags, in the order every parser spec merges them.
pub(crate) static GLOBAL_BOOL_FLAGS: &[Flag] = &[GLOBAL_NO_INPUT, GLOBAL_YES];

/// The two `compose` records, registered as the reserved surfaces' tranche's
/// tail so the surface group covers all 17 reserved leaves.
static COMPOSE_LEAVES: &[CommandSpec] =
    &[surface::compose::COMPOSE_UP, surface::compose::COMPOSE_DOWN];

/// The command tranches, in registration order.
///
/// `all` walks them in this order; both rendered help surfaces read their verb
/// order from `cli_command_bindings` instead.
static TRANCHES: &[&[CommandSpec]] = &[
    core_reads::CORE_READS,
    task_history::TASK_HISTORY,
    graph_records::GRAPH_RECORDS,
    persona_local::PERSONA_LOCAL,
    shared_workspace::SHARED_WORKSPACE,
    core_writes::CORE_WRITES,
    write_operations::WRITE_OPERATIONS,
    surface::SURFACE,
    COMPOSE_LEAVES,
    backup_leaves::BACKUP_LEAVES,
    sync_leaves::SYNC_LEAVES,
    auth_config::AUTH_CONFIG_LEAVES,
    ranking_retention::RANKING_RETENTION_LEAVES,
    compliance_leaves::COMPLIANCE_LEAVES,
];

/// Every global flag name that takes a value, in merge order.
pub(crate) fn global_value_names() -> impl Iterator<Item = &'static str> {
    GLOBAL_VALUE_FLAGS.iter().map(|flag| flag.long)
}

/// Every boolean global flag name, in merge order.
pub(crate) fn global_bool_names() -> impl Iterator<Item = &'static str> {
    GLOBAL_BOOL_FLAGS.iter().map(|flag| flag.long)
}

/// The global flag with this name, when one is declared.
pub(crate) fn global_flag(name: &str) -> Option<&'static Flag> {
    GLOBAL_VALUE_FLAGS
        .iter()
        .chain(GLOBAL_BOOL_FLAGS.iter())
        .find(|flag| flag.long == name)
}

/// Every command record, tranche order preserved.
pub(crate) fn all() -> impl Iterator<Item = &'static CommandSpec> {
    TRANCHES.iter().flat_map(|tranche| tranche.iter())
}

/// The visible record with this name, when one is registered.
pub(crate) fn find(name: &str) -> Option<&'static CommandSpec> {
    all().find(|spec| spec.name == name)
}

/// The record this name reaches: a visible name first, else an alias.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn resolve(name: &str) -> Option<&'static CommandSpec> {
    resolve_in(all(), name)
}

/// The record this name reaches in `specs`: the first record whose name
/// matches, else the first record whose aliases contain it.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn resolve_in<'a>(
    specs: impl Iterator<Item = &'a CommandSpec>,
    name: &str,
) -> Option<&'a CommandSpec> {
    let mut aliased: Option<&'a CommandSpec> = None;
    for spec in specs {
        if spec.name == name {
            return Some(spec);
        }
        if aliased.is_none() && spec.aliases.contains(&name) {
            aliased = Some(spec);
        }
    }
    aliased
}

/// Every visible command name, tranche order preserved.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn names() -> impl Iterator<Item = &'static str> {
    all().map(|spec| spec.name)
}

/// Every declared alias as `(alias, canonical name)`, tranche order preserved.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn aliases() -> impl Iterator<Item = (&'static str, &'static str)> {
    all().flat_map(|spec| spec.aliases.iter().map(move |alias| (*alias, spec.name)))
}

/// The static-data contract over one set of records, kept in its own module
/// because this file is at its size limit; re-exported here so every caller
/// keeps the import path it already reads.
#[cfg_attr(not(test), allow(unused_imports))]
pub(crate) use crate::cli_command_registry_contract::{
    check_flag_metadata, check_metadata, validate,
};

// `pub(crate)` so the static-data contract in `cli_command_registry_contract`
// can run `examples::check_examples` over the records it validates.
#[path = "cli_command_registry_examples.rs"]
pub(crate) mod examples;

#[path = "cli_command_registry_help_topics.rs"]
mod help_topics;

#[cfg(test)]
#[path = "cli_command_registry_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "cli_command_registry_tranche_tests.rs"]
mod tranche_tests;

#[cfg(test)]
#[path = "cli_command_registry_bank_tests.rs"]
mod bank_tests;
