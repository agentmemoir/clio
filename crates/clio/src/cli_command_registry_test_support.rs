// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the shared record lookup used by the
//! `cli_command_registry_*` test suites.
//!
//! ## Owns
//! - `record`, the registered-record fetch that fails naming the missing command
//!
//! ## Does not own
//! - The records themselves (see the `cli_command_registry_*` modules)
//! - The per-suite assertions over them
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the one shared registry lookup.

use crate::cli_command_spec::CommandSpec;

/// One registered record, or a failure that names the missing command.
pub(crate) fn record(name: &str) -> &'static CommandSpec {
    crate::cli_command_registry::find(name).unwrap_or_else(|| panic!("`{name}` is registered"))
}
