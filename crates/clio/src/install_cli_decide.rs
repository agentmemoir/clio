// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for turning the arguments of `clio install` and
//! `clio uninstall` into the decision the surface then runs: help, the
//! portable-only skill write, the skill-only removal, or one harness record with
//! its `--force` flag.
//!
//! ## Owns
//! - The two verbs and the labels and purposes their help prints
//! - The decision type: help, portable-only install, skill-only removal, or one
//!   harness record
//! - The parse-and-decide step, which reads the verb's own registry record, so
//!   `--force` is accepted on `install` and refused on `uninstall`
//!
//! ## Does not own
//! - The help and usage-error text the decision carries (`install_cli_help`)
//! - Running the decision, the delegate calls, and the exit codes
//!   (`install_cli`)
//! - Writing the skill package (`skill_install`) or removing it
//!   (`skill_uninstall`)
//! - Which harnesses exist, their names, and their argv shapes
//!   (`agent_harness`)
//!
//! ## Boundary
//!
//! Changes to which flags these verbs accept, or to how a flag maps to a
//! decision, should normally require changes here. Changes to what a decision
//! then does should not.
//!
//! The step is pure: no executable lookup, no delegate call, and no output, so a
//! test can decide an outcome without spawning a process.
//!
//! Keep this module focused on the parse-and-decide step of the two verbs.

use crate::agent_harness::{self, HarnessRecord};
use crate::cli_error::CliError;
use crate::cli_top_level::{parse_flags, record_flag_names, wants_help};
use crate::install_cli_help::{known_but_unsupported, missing_harness, unknown_harness};

/// The two verbs, so the parse-decide-report flow runs once for both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Verb {
    Install,
    Uninstall,
}

impl Verb {
    /// The command name a user sees.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Verb::Install => "install",
            Verb::Uninstall => "uninstall",
        }
    }

    /// What the verb does, for the help text.
    pub(crate) fn purpose(self) -> &'static str {
        match self {
            Verb::Install => "Register the clio MCP server with a supported agent harness.",
            Verb::Uninstall => {
                "Remove the clio MCP server registration from a supported agent harness."
            }
        }
    }
}

/// What one verb should do after parsing.
#[derive(Debug)]
pub(crate) enum Decision {
    Help,
    /// `clio install` with no `--harness`: write the portable skill package only
    /// and skip server registration. `force` replaces the assets clio recorded.
    Portable {
        force: bool,
    },
    /// `clio uninstall --skill-only`: remove the installed skill package only and
    /// leave the harness registration alone. `None` targets the portable root.
    SkillOnly {
        record: Option<&'static HarnessRecord>,
    },
    /// One harness to run, with whether a conflicting skill package may be
    /// replaced.
    Run {
        record: &'static HarnessRecord,
        force: bool,
    },
}

/// Parse the arguments and decide what the verb should do. A missing `--harness`
/// is the portable-only install when the verb is install, the portable skill
/// removal under `--skill-only`, and a usage error otherwise.
///
/// # Errors
/// An unknown flag, a flag with no value, an unknown harness, or a missing
/// `--harness` on uninstall outside `--skill-only`.
pub(crate) fn decide(verb: Verb, args: &[String]) -> Result<Decision, CliError> {
    if wants_help(args) {
        return Ok(Decision::Help);
    }
    // Each verb parses its own record: `install` documents `--force` and
    // `uninstall` does not, so `--force` on uninstall is a usage error rather
    // than a silently ignored token.
    let flags = parse_flags(args.iter().cloned(), &record_flag_names(verb.label()))?;
    if flags.contains_key("help") {
        return Ok(Decision::Help);
    }
    let force = flags.contains_key("force");
    if flags.contains_key("skill-only") {
        let record = match flags.get("harness") {
            Some(name) => Some(resolve_harness(name)?),
            None => None,
        };
        return Ok(Decision::SkillOnly { record });
    }
    let name = match flags.get("harness") {
        Some(name) => name.as_str(),
        None if verb == Verb::Install => return Ok(Decision::Portable { force }),
        None => return Err(missing_harness()),
    };
    Ok(Decision::Run {
        record: resolve_harness(name)?,
        force,
    })
}

/// The harness record for an accepted name, with the shared errors for a name
/// that is unknown or known but not supported.
fn resolve_harness(name: &str) -> Result<&'static HarnessRecord, CliError> {
    match agent_harness::resolve(name) {
        Some(record) => Ok(record),
        None => match agent_harness::normalize(name) {
            Some(canonical) => Err(known_but_unsupported(canonical)),
            None => Err(unknown_harness(name)),
        },
    }
}

#[cfg(test)]
#[path = "install_cli_decide_tests.rs"]
mod tests;
