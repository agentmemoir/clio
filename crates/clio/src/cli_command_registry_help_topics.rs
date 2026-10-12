// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the static contract that every visible
//! record name resolves as a `clio help <topic>`, through the same top-level
//! lookup the CLI uses.
//!
//! ## Owns
//! - `check_help_topics`, the pure per-name topic check
//!
//! ## Does not own
//! - The topic parsing itself (see `cli_top_level::help_topic`)
//! - The known-command and group tables (see `cli_help`)
//! - The usage line a topic prints (see `cli_help_usage`)
//!
//! ## Boundary
//!
//! Changes to what makes a name a resolvable help topic should normally require
//! changes here. Changes to one command's usage text should not require changes
//! here.
//!
//! Keep this module focused on the help-topic contract.

use crate::cli_command_spec::CommandSpec;

/// Check every record name resolves as a `clio help <topic>` — the top level's
/// own gate, which accepts multi-token names like `task get` and the aliased
/// spelling `show` — and that a usage line exists to print.
///
/// The check drives the same pure entry point `clio help <topic>` runs, so the
/// whole registry is covered without spawning a child process per record, and
/// a name that fails is named in the message.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn check_help_topics(specs: &[&CommandSpec]) -> Result<(), String> {
    for spec in specs {
        let args = [spec.name.to_owned()];
        match crate::cli_top_level::help_topic(&args) {
            Ok(Some(_)) => {}
            Ok(None) => {
                return Err(format!(
                    "`{}` does not resolve as a `clio help` topic",
                    spec.name
                ));
            }
            Err(error) => {
                return Err(format!(
                    "`{}` does not resolve as a `clio help` topic: {}",
                    spec.name,
                    error.message()
                ));
            }
        }
        if crate::cli_help_usage::usage_for(spec.name).is_none() {
            return Err(format!(
                "`{}` resolves as a `clio help` topic but has no usage line",
                spec.name
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "cli_command_registry_help_topics_tests.rs"]
mod tests;
