// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the help and success text of the
//! `clio install` and `clio uninstall` surfaces, and for the supported-harness
//! usage errors they share.
//!
//! ## Owns
//! - The supported-harness hint, generated from the registry
//! - The usage errors for a missing, unknown, or known-but-unsupported harness
//! - The full help text, including the portable-only default for install and
//!   what `--force` replaces
//! - The success text, including a harness's config-surface note
//!
//! ## Does not own
//! - Parsing flags or deciding what a verb runs (`install_cli_decide`)
//! - Registering the MCP server or writing skill assets
//! - Which harnesses exist, their names, and their config files
//!   (`agent_harness`)
//!
//! ## Boundary
//!
//! Changes to the words these commands print should normally require changes
//! here. Changes to how the commands run should not.
//!
//! Keep this module focused on the install and uninstall text.

use crate::agent_harness::{self, HarnessRecord};
use crate::cli_error::CliError;
use crate::install_cli_decide::Verb;

/// The supported-harness list, generated from the registry so the help and the
/// usage errors cannot drift apart.
pub(crate) fn supported_hint() -> String {
    format!(
        "supported harnesses: {}",
        agent_harness::supported_names().join(", ")
    )
}

/// A call with no `--harness` flag on a verb that requires it.
pub(crate) fn missing_harness() -> CliError {
    CliError::usage("--harness is required").with_hint(supported_hint())
}

/// A call naming a harness the registry does not know.
pub(crate) fn unknown_harness(name: &str) -> CliError {
    CliError::usage(format!("unknown harness `{name}`")).with_hint(supported_hint())
}

/// A call naming a harness the registry recognizes by name but does not yet
/// implement, such as `cursor` or the Copilot alias `vs code`. The name is
/// understood, so the refusal says so rather than calling it unknown.
pub(crate) fn known_but_unsupported(canonical: &str) -> CliError {
    CliError::usage(format!(
        "harness `{canonical}` is known but not yet supported"
    ))
    .with_hint(supported_hint())
}

/// The config-surface note for a harness whose entry lands in a vendor-owned
/// file the user may not expect. The delegate writes the Antigravity CLI
/// global config, so the note names that file. Only Antigravity has a note
/// today, so no other harness gets one.
pub(crate) fn config_surface_note(record: &HarnessRecord) -> Option<String> {
    (record.name == "antigravity").then(|| {
        format!(
            "note: antigravity writes the CLI global config (~/{})",
            record.config_file
        )
    })
}

/// The text a successful install or uninstall prints for this harness: the
/// delegate's message, with the config-surface note appended when the harness
/// has one. Pure, so a test can check the note without capturing stdout.
pub(crate) fn success_text(record: &HarnessRecord, message: &str) -> String {
    match config_surface_note(record) {
        Some(note) => format!("{message}\n{note}"),
        None => message.to_owned(),
    }
}

/// What `--force` replaces, and what it never touches. The paragraph is part of
/// the install help because the refusal a conflicting package earns points a
/// user here.
const FORCE_PARAGRAPH: &str = "--force replaces the skill assets clio recorded, and only those: without it, a locally edited asset, a missing asset, or an unexpected file in the package directory refuses the whole install. It never deletes an unowned file and never deletes a skill root.";

/// The surface help: the usage line, what the verb does, the supported list
/// from the registry, and one example.
pub(crate) fn print_help(verb: Verb) {
    println!("{}", help_text(verb));
}

/// The full help text, split from `print_help` so a test can read it without
/// capturing stdout. Install states that a call with no `--harness` writes only
/// the portable skill package and skips registration, and what `--force`
/// replaces. The Antigravity config-surface note is included because help lists
/// every supported harness, and an Antigravity user needs to know which file
/// received the entry.
pub(crate) fn help_text(verb: Verb) -> String {
    let usage = crate::cli_help_usage::usage_line(verb.label()).unwrap_or_else(|| match verb {
        Verb::Install => "clio install [--harness <name>] [--force]".to_owned(),
        Verb::Uninstall => "clio uninstall --harness <name>".to_owned(),
    });
    let mut text = format!("{usage}\n\n{}\n\n", verb.purpose());
    if verb == Verb::Install {
        text.push_str(
            "With no --harness, write the portable skill package to ~/.agents/skills/clio-mcp and skip server registration.\n\n",
        );
        text.push_str(FORCE_PARAGRAPH);
        text.push_str("\n\n");
    }
    text.push_str(&supported_hint());
    text.push_str("\n\nExample: clio ");
    text.push_str(verb.label());
    text.push_str(" --harness claude");
    if let Some(note) = agent_harness::resolve("antigravity").and_then(config_surface_note) {
        text.push_str("\n\n");
        text.push_str(&note);
    }
    text
}

#[cfg(test)]
#[path = "install_cli_help_tests.rs"]
mod tests;
