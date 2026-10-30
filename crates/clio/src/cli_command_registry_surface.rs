// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the reserved-surface tranche: the `SURFACE`
//! list that fixes the fifteen records' catalog order, the flag facts the
//! record modules share, and the child modules the records live in.
//!
//! ## Owns
//! - The `SURFACE` tranche, in catalog order: the seven single commands, then
//!   the eight `mcp` and `ops` subcommands
//! - The shared flag facts its children and the compose records read:
//!   `RESERVED_GLOBALS`, `DB_BANK_UNDOCUMENTED_GLOBALS`, and
//!   `DB_BACKEND_BANK`
//! - The child modules: the seven single commands (`singles`), the three
//!   `mcp` subcommands (`mcp`), the five `ops` subcommands (`ops`), the two
//!   `compose` records (`compose`), and the accepted-name derivation the
//!   reserved surfaces parse with (`flags`)
//!
//! ## Does not own
//! - The records themselves (see the `singles`, `mcp`, and `ops` children)
//! - The record model or its rendering (see `cli_command_spec`)
//! - The registry and the shared global flags (see `cli_command_registry`)
//! - The surfaces' own dispatch and the prose each surface prints beside its
//!   record (see `cli_top_level`, `status_cli`, `setup_cli`, `install_cli`,
//!   `mcp_cli`, `ops_cli`, and `compose_cli`)
//!
//! ## Boundary
//!
//! Changes to one of these commands' flags, usage line, or summary text should
//! normally require changes in the child module that owns the record. Changes
//! to the tranche order or the shared flag facts should normally require
//! changes here. Changes to how a surface dispatches should not.
//!
//! Keep this module focused on the tranche list and the flag facts the
//! records share.

use crate::cli_command_registry::{GLOBAL_BACKEND, GLOBAL_BANK, GLOBAL_DB, GLOBAL_OUTPUT};
use crate::cli_command_spec::{CommandSpec, Flag};

/// The globals a reserved surface accepts without printing: the shared parser
/// accepts every declared global on every surface, so each record it gates
/// that documents none of them lists all seven here. `help` and `setup` list
/// none: their dispatches read only their documented flags.
pub(crate) const RESERVED_GLOBALS: &[&str] = &[
    "db", "backend", "bank", "actor", "output", "no-input", "yes",
];

/// The declared globals a record that documents `--db`, `--backend`, and
/// `--bank` still accepts without printing.
const DB_BANK_UNDOCUMENTED_GLOBALS: &[&str] = &["actor", "output", "no-input", "yes"];

/// The declared globals a record that also documents `--output` still accepts
/// without printing: the same set as `DB_BANK_UNDOCUMENTED_GLOBALS` with
/// `output` removed, because the record contract refuses a flag a record both
/// accepts and documents.
const DB_BANK_UNDOCUMENTED_GLOBALS_NO_OUTPUT: &[&str] = &["actor", "no-input", "yes"];

/// The trio the `mcp` and `ops` subcommands print: the global value flags
/// every one of them names on its usage line.
const DB_BACKEND_BANK: &[Flag] = &[GLOBAL_DB, GLOBAL_BACKEND, GLOBAL_BANK];

/// The trio plus the output mode: a record whose report renders through the
/// shared `--output` mode names it on its usage line.
const DB_BACKEND_BANK_OUTPUT: &[Flag] = &[GLOBAL_DB, GLOBAL_BACKEND, GLOBAL_BANK, GLOBAL_OUTPUT];

/// The reserved-surface records: the seven single commands, then the eight
/// `mcp` and `ops` subcommands, each group in its catalog order.
pub(crate) static SURFACE: &[CommandSpec] = &[
    VERSION,
    HEALTH,
    HELP,
    STATUS,
    SETUP,
    INSTALL,
    UNINSTALL,
    MCP_SCHEMA_EXPORT,
    MCP_STDIO,
    MCP_HTTP,
    OPS_DIAGNOSE,
    OPS_VERIFY,
    OPS_DOCTOR,
    OPS_REPAIR,
    OPS_REINDEX,
];

/// The seven single commands: `version`, `health`, `help`, `status`, `setup`,
/// `install`, and `uninstall`.
#[path = "cli_command_registry_surface_singles.rs"]
pub(crate) mod singles;

/// The three `mcp` subcommands: the schema export and the two transports.
#[path = "cli_command_registry_surface_mcp.rs"]
pub(crate) mod mcp;

/// The five `ops` subcommands: diagnose, verify, doctor, repair, and reindex.
#[path = "cli_command_registry_surface_ops.rs"]
pub(crate) mod ops;

/// The `compose up` and `compose down` records, kept beside the reserved
/// surfaces' records.
#[path = "cli_command_registry_surface_compose.rs"]
pub(crate) mod compose;

/// The accepted-name derivation the reserved surfaces' parsers gate on.
#[path = "cli_command_registry_surface_flags.rs"]
pub(crate) mod flags;

pub(crate) use compose::{COMPOSE_DOWN, COMPOSE_UP};
pub(crate) use flags::parse_flag_names;
pub(crate) use mcp::{MCP_HTTP, MCP_SCHEMA_EXPORT, MCP_STDIO};
pub(crate) use ops::{OPS_DIAGNOSE, OPS_DOCTOR, OPS_REINDEX, OPS_REPAIR, OPS_VERIFY};
pub(crate) use singles::{HEALTH, HELP, INSTALL, SETUP, STATUS, UNINSTALL, VERSION};

#[cfg(test)]
#[path = "cli_command_registry_surface_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "cli_command_registry_surface_parse_tests.rs"]
mod parse_tests;
