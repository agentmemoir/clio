// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the words the skill installer prints: the outcome line for each root, the refusal that names every conflicting path, and the plain failure message for a write that could not happen.
//!
//! ## Owns
//! - The written, unchanged, and refused outcome text, one line per root
//! - The reason text for every conflict a root reports
//! - The portable-root reason line, and the failure messages for a skill write
//! - The single question the installer asks of an outcome: did it refuse
//!
//! ## Does not own
//! - Deciding that a root is current, written, or in conflict (`skill_install_plan`)
//! - Writing any file (`skill_install`)
//! - The embedded asset table (`clio_mcp::skill_assets`)
//! - Which harness has a portable-root note (`agent_harness`)
//!
//! ## Boundary
//!
//! Changes to what these commands print should normally require changes here. Changes to what they do should not.
//!
//! The text never claims a client loaded the skill, and never claims a write the installer did not make: a root with nothing to write says so, and a written root names every file it wrote.
//!
//! Keep this module focused on the skill installer's text.

use crate::agent_harness::HarnessRecord;
use crate::agent_harness_write::WriteError;
use crate::skill_install::{RootConflict, SkillOutcome, SkillRootOutcome};
use crate::skill_install_plan::ConflictReason;
use clio_mcp::skill_assets::SKILL_ASSETS;
use std::path::Path;

/// The hermes reason line: its client has no documented native skill mechanism, so it receives the portable copy.
const PORTABLE_REASON: &str =
    "note: hermes has no documented native skill mechanism, so the portable skill root is used";

/// A note for a marker this program cannot read, which `--force` does not override: no record means no way to tell what this program owns.
const UNREADABLE_MARKER_NOTE: &str = "a marker that cannot be read is refused even with --force, because nothing proves which files clio owns; delete it by hand to install the package again";

/// The skill outcome text, one line per root plus whether anything changed there. A harness that receives the portable copy also gets the reason line.
pub(crate) fn outcome_text(record: &HarnessRecord, outcome: &SkillOutcome) -> String {
    let lines = outcome_lines(outcome);
    match portable_reason(record) {
        Some(reason) => format!("{lines}\n{reason}"),
        None => lines,
    }
}

/// The outcome text for the portable-only install, which names no harness.
pub(crate) fn portable_outcome_text(outcome: &SkillOutcome) -> String {
    outcome_lines(outcome)
}

/// Whether `outcome` refused every write, so the caller must fail rather than report success.
pub(crate) fn refused(outcome: &SkillOutcome) -> bool {
    matches!(outcome, SkillOutcome::Refused { .. })
}

/// One line per root, so every directory is named.
fn outcome_lines(outcome: &SkillOutcome) -> String {
    match outcome {
        SkillOutcome::Unchanged { roots } => roots
            .iter()
            .map(|dir| current_line(dir))
            .collect::<Vec<_>>()
            .join("\n"),
        SkillOutcome::Written { roots } => {
            roots.iter().map(root_lines).collect::<Vec<_>>().join("\n")
        }
        SkillOutcome::Refused { conflicts } => refusal_text(conflicts),
    }
}

/// The line for a root that was already current: the asset count, the word unchanged, and the fact that nothing was written.
fn current_line(dir: &Path) -> String {
    format!(
        "{} skill assets already current in {}; unchanged, nothing written",
        SKILL_ASSETS.len(),
        dir.display()
    )
}

/// The lines for one written root: every file it wrote, then every unowned file it kept.
fn root_lines(root: &SkillRootOutcome) -> String {
    let mut lines = Vec::with_capacity(2);
    if root.written.is_empty() {
        lines.push(current_line(&root.dir));
    } else {
        lines.push(format!(
            "installed {} skill assets into {}; wrote {}",
            SKILL_ASSETS.len(),
            root.dir.display(),
            root.written.join(", ")
        ));
    }
    if !root.preserved_foreign.is_empty() {
        lines.push(format!(
            "kept unowned files in {}: {}",
            root.dir.display(),
            root.preserved_foreign.join(", ")
        ));
    }
    lines.join("\n")
}

/// The refusal text: every conflicting root, every path, and every reason, then what `--force` does and does not do.
fn refusal_text(conflicts: &[RootConflict]) -> String {
    let mut lines = vec![format!(
        "refused to write the clio-mcp skill package because {} skill root{} hold{} paths clio cannot vouch for:",
        conflicts.len(),
        if conflicts.len() == 1 { "" } else { "s" },
        if conflicts.len() == 1 { "s" } else { "" }
    )];
    for conflict in conflicts {
        lines.push(format!("  {}:", conflict.dir.display()));
        for reason in &conflict.reasons {
            lines.push(format!("    {}: {}", reason.path(), reason_text(reason)));
        }
    }
    lines.push(String::from(
        "nothing was written in any skill root; re-run with --force to replace the assets clio recorded",
    ));
    lines.push(String::from(
        "--force replaces owned assets only: it never deletes an unowned file and never deletes a skill root",
    ));
    if conflicts.iter().any(|conflict| {
        conflict
            .reasons
            .iter()
            .any(|reason| matches!(reason, ConflictReason::UnreadableMarker { .. }))
    }) {
        lines.push(String::from(UNREADABLE_MARKER_NOTE));
    }
    lines.join("\n")
}

/// What one conflict reason means, in the words the refusal prints.
fn reason_text(reason: &ConflictReason) -> &'static str {
    match reason {
        ConflictReason::LocalEdit { .. } => "local edit",
        ConflictReason::MissingOwned { .. } => "missing owned file",
        ConflictReason::UnexpectedFile { .. } => "unexpected file",
        ConflictReason::Unrecorded { .. } => "unrecorded package",
        ConflictReason::UnreadableMarker { .. } => "unreadable marker",
    }
}

/// The plain failure message for a skill write: the harness, the failing path, and the cause. `WriteError` already carries the path in its detail.
pub(crate) fn failure_message(record: &HarnessRecord, error: &WriteError) -> String {
    match error {
        WriteError::Io { detail } => {
            format!(
                "could not install the {} skill assets: {detail}",
                record.name
            )
        }
        WriteError::Unparseable { path, snippet } => format!(
            "could not install the {} skill assets: {} {snippet}",
            record.name,
            path.display()
        ),
    }
}

/// The plain failure message for the portable-only write: the failing path and the cause. `WriteError` already carries the path in its detail.
pub(crate) fn portable_failure_message(error: &WriteError) -> String {
    match error {
        WriteError::Io { detail } => {
            format!("could not install the portable clio-mcp skill assets: {detail}")
        }
        WriteError::Unparseable { path, snippet } => format!(
            "could not install the portable clio-mcp skill assets: {} {snippet}",
            path.display()
        ),
    }
}

/// The portable-copy reason for a harness that has no documented native skill mechanism.
fn portable_reason(record: &HarnessRecord) -> Option<&'static str> {
    (record.name == "hermes").then_some(PORTABLE_REASON)
}

#[cfg(test)]
#[path = "skill_install_text_tests.rs"]
mod tests;
