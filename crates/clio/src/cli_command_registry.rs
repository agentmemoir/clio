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
//! - `validate`, the static-data contract over one set of records
//! - `check_flag_metadata` / `check_metadata`, the flag-documentation contract
//!
//! ## Does not own
//! - The command records themselves (see the `cli_command_registry_*` record
//!   modules)
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
use crate::cli_command_spec::{CommandSpec, Flag, ValueKind};

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

/// Check one set of records against the catalog inventory.
///
/// Rejects, naming the offender: a duplicate visible name, a visible name
/// absent from `inventory`, an empty or self-referential alias, an alias two
/// records claim, an alias that shadows a different-tool visible record, two
/// records that alias each other, and an `accepted_extra` name that is not a
/// declared global flag or that the same record also documents.
///
/// A flag documented twice by one record is accepted, because a real command
/// prints a required and an optional occurrence of the same flag. An alias
/// that names no visible record is accepted as a plain extra spelling.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn validate(specs: &[&CommandSpec], inventory: &[&str]) -> Result<(), String> {
    let mut seen: Vec<&str> = Vec::new();
    for spec in specs {
        if seen.contains(&spec.name) {
            return Err(format!("`{}` is registered twice", spec.name));
        }
        if !inventory.contains(&spec.name) {
            return Err(format!("`{}` is not in the catalog inventory", spec.name));
        }
        for name in spec.accepted_extra {
            if global_flag(name).is_none() {
                return Err(format!(
                    "`{}` accepts `{name}` but it is not a declared global flag",
                    spec.name
                ));
            }
            if spec.documented.iter().any(|flag| flag.long == *name) {
                return Err(format!(
                    "`{}` accepts `{name}` and also documents it",
                    spec.name
                ));
            }
        }
        seen.push(spec.name);
    }
    let mut claimed: Vec<(&'static str, &CommandSpec)> = Vec::new();
    for spec in specs {
        for alias in spec.aliases {
            if alias.is_empty() {
                return Err(format!("`{}` declares an empty alias", spec.name));
            }
            if *alias == spec.name {
                return Err(format!("`{}` declares its own name as an alias", spec.name));
            }
            if let Some(owner) = claimed
                .iter()
                .find(|(name, claim)| *name == *alias && claim.name != spec.name)
            {
                return Err(format!(
                    "alias `{alias}` is claimed by both `{}` and `{}`",
                    owner.1.name, spec.name
                ));
            }
            claimed.push((*alias, spec));
        }
    }
    for (alias, spec) in &claimed {
        if let Some(target) = specs.iter().find(|candidate| candidate.name == *alias) {
            if target.tool != spec.tool {
                return Err(format!(
                    "`{}` shadows `{}` with alias `{alias}`",
                    spec.name, target.name
                ));
            }
            if target.aliases.contains(&spec.name) {
                return Err(format!(
                    "`{}` and `{}` alias each other in a cycle",
                    spec.name, target.name
                ));
            }
        }
    }
    Ok(())
}

/// Check one flag's documentation metadata is complete.
///
/// Rejects a blank description, a value flag with a blank placeholder, and an
/// `Enum` value kind whose allowed-value list is empty. The metadata is for
/// readers of the catalog; it never reaches the parser, so a bad record is a
/// documentation defect, not a grammar change.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn check_flag_metadata(flag: &Flag) -> Result<(), String> {
    if flag.description.trim().is_empty() {
        return Err(format!("flag `--{}` has no description", flag.long));
    }
    if let Some(value) = flag.value {
        if value.placeholder.trim().is_empty() {
            return Err(format!("flag `--{}` has a blank placeholder", flag.long));
        }
        if let ValueKind::Enum(allowed) = value.kind
            && allowed.is_empty()
        {
            return Err(format!(
                "flag `--{}` has an empty allowed-value list",
                flag.long
            ));
        }
    }
    Ok(())
}

/// Check the flag metadata of every spec's documented flags and the declared
/// globals, naming the record that owns the bad flag.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn check_metadata(specs: &[&CommandSpec]) -> Result<(), String> {
    for spec in specs {
        for flag in spec.documented {
            check_flag_metadata(flag).map_err(|error| format!("`{}`: {error}", spec.name))?;
        }
    }
    for flag in GLOBAL_VALUE_FLAGS.iter().chain(GLOBAL_BOOL_FLAGS.iter()) {
        check_flag_metadata(flag).map_err(|error| format!("global {error}"))?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "cli_command_registry_tests.rs"]
mod tests;
