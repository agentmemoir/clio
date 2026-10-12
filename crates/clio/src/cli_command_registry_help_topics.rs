// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module owns the help-topic contract test: every visible record name
//! resolves as a `clio help <topic>`, through the same top-level lookup the CLI
//! uses, and a usage line exists to print.
//!
//! The check is a test helper and lives with this module's tests, so the
//! production build carries no test-only code.
//!
//! ## Does not own
//! - The topic parsing itself (see `cli_top_level::help_topic`)
//! - The known-command and group tables (see `cli_help`)
//! - The usage line a topic prints (see `cli_help_usage`)
//!
//! ## Boundary
//!
//! Changes to what makes a name a resolvable help topic should normally require
//! changes here and in the tests beside it. Changes to one command's usage text
//! should not require changes here.
//!
//! Keep this module focused on the help-topic contract.

#[cfg(test)]
#[path = "cli_command_registry_help_topics_tests.rs"]
mod tests;
