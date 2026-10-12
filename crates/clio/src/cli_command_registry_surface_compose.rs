// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the static records of the two `clio compose`
//! subcommands, `up` and `down`.
//!
//! ## Owns
//! - The `compose up` and `compose down` `CommandSpec` records
//!
//! ## Does not own
//! - The reserved-surface records and the tranche that lists them (see
//!   `cli_command_registry_surface`)
//! - The registry and its tranche order (see `cli_command_registry`)
//! - The compose dispatch and its pipelines (see `compose_cli`)
//!
//! ## Boundary
//!
//! Changes to a compose subcommand's flags or summary text should normally
//! require changes here. Changes to what compose runs should not require
//! changes here.
//!
//! Keep this module focused on the compose records.

use super::RESERVED_GLOBALS;
use crate::cli_command_spec::{CommandSpec, Example, Flag, Section};

/// The compose root override.
const DIR: Flag = Flag::text("dir", "DIR", "Compose root `compose_cli` resolves.");
/// The service selection.
const SERVICES: Flag = Flag::text("services", "LIST", "Service selection `compose_cli` takes.");

/// `clio compose up`: materialize the stack and start the chosen services.
pub(crate) const COMPOSE_UP: CommandSpec = CommandSpec {
    name: "compose up",
    aliases: &[],
    summary: "Materialize the compose stack and start the local Docker services (postgres, embed, rerank, extract)",
    notes: &[],
    positionals: &[],
    documented: &[DIR, SERVICES],
    accepted_extra: RESERVED_GLOBALS,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[
        Example {
            command: "clio compose up --services postgres",
            comment: Some("Start only the Postgres service."),
        },
        Example {
            command: "clio compose up --services postgres,embed,rerank",
            comment: Some("Start the local-retrieval stack."),
        },
        Example {
            command: "clio compose up --dir ./deploy/compose --services postgres,embed,rerank",
            comment: Some("Materialize under an explicit compose root."),
        },
    ],
    tool: None,
    section: Section::Write,
};

/// `clio compose down`: stop and remove the services `up` started.
pub(crate) const COMPOSE_DOWN: CommandSpec = CommandSpec {
    name: "compose down",
    aliases: &[],
    summary: "Stop and remove the services `clio compose up` started; re-runnable",
    notes: &[],
    positionals: &[],
    documented: &[DIR, SERVICES],
    accepted_extra: RESERVED_GLOBALS,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[
        Example {
            command: "clio compose down --services postgres",
            comment: Some("Stop an explicit service selection."),
        },
        Example {
            command: "clio compose down",
            comment: Some("Stop the selection the last up recorded."),
        },
        Example {
            command: "clio compose down --dir ./deploy/compose --services postgres,embed,rerank",
            comment: Some("Stop the stack under an explicit compose root."),
        },
    ],
    tool: None,
    section: Section::Write,
};

#[cfg(test)]
#[path = "cli_command_registry_surface_compose_tests.rs"]
mod tests;
