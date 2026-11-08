// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the words `clio uninstall --skill-only` prints: the outcome line for each package directory, the refusal that names every fact which does not match, and the plain failure message for a removal that could not run.
//!
//! ## Owns
//! - The removed, retained, and no-op outcome text, one line per directory
//! - The refusal header, one line per reason, and the closing explanation
//! - The dedicated message for a marker that records another installer
//! - The reason text for every refusal fact
//!
//! ## Does not own
//! - Deciding which directory is removed, retained, or a no-op (`skill_uninstall`)
//! - What the marker records (`skill_marker`)
//! - The installer's own text (`skill_install_text`)
//!
//! ## Boundary
//!
//! Changes to what this command prints should normally require changes here. Changes to what it does should not.
//!
//! The text never claims a removal that did not happen: a retained package names the harnesses that still reference it, and a refusal states that nothing was removed.
//!
//! Keep this module focused on the skill remover's text.

use crate::skill_install_plan::ConflictReason;
use crate::skill_uninstall::{RefusalReason, RemovalOutcome, UninstallError};
use std::path::Path;

/// One line per directory the run inspected.
pub(crate) fn outcome_text(outcomes: &[RemovalOutcome]) -> String {
    outcomes
        .iter()
        .map(outcome_line)
        .collect::<Vec<_>>()
        .join("\n")
}

/// The plain failure message for a removal that stopped.
pub(crate) fn failure_message(error: &UninstallError) -> String {
    match error {
        UninstallError::Refused { dir, reasons } => refusal_text(dir, reasons),
        UninstallError::Io { detail } => {
            format!("could not remove the clio-mcp skill package: {detail}")
        }
    }
}

/// The line for one directory: what was removed, what was kept, or that nothing was there.
fn outcome_line(outcome: &RemovalOutcome) -> String {
    match outcome {
        RemovalOutcome::Removed {
            dir,
            files,
            removed_dir,
        } => {
            let mut line = format!(
                "removed the clio-mcp skill package from {}; removed {} files and the marker",
                dir.display(),
                files.len()
            );
            if *removed_dir {
                line.push_str("; removed the package directory");
            }
            line
        }
        RemovalOutcome::Retained { dir, remaining } => format!(
            "retained the clio-mcp skill package in {}; still referenced by: {}",
            dir.display(),
            remaining.iter().cloned().collect::<Vec<_>>().join(", ")
        ),
        RemovalOutcome::NoOp { dir } => {
            format!(
                "no clio-mcp skill package in {}; nothing to remove",
                dir.display()
            )
        }
    }
}

/// The refusal text: a foreign installer names itself, and every other refusal lists each path, each reason, and the closing rule.
fn refusal_text(dir: &Path, reasons: &[RefusalReason]) -> String {
    if let Some(RefusalReason::ForeignInstaller { recorded }) = reasons
        .iter()
        .find(|reason| matches!(reason, RefusalReason::ForeignInstaller { .. }))
    {
        return format!(
            "refused to remove {}: its marker records installer `{recorded}`, not `clio`",
            dir.display()
        );
    }
    let mut lines = vec![format!(
        "refused to remove the clio-mcp skill package because {} recorded facts do not match:",
        reasons.len()
    )];
    for reason in reasons {
        lines.push(reason_line(reason));
    }
    lines.push(String::from(
        "nothing was removed; clio removes only a package whose marker records installer clio, the payload digest, and every file's bytes",
    ));
    lines.join("\n")
}

/// One refusal line. A payload-digest reason also names both digests, so the two values in the value type reach the user.
fn reason_line(reason: &RefusalReason) -> String {
    match reason {
        RefusalReason::PayloadDigest { recorded, found } => format!(
            "    {}: {} (recorded {recorded}, found {found})",
            reason.path(),
            reason_text(reason)
        ),
        other => format!("    {}: {}", other.path(), reason_text(other)),
    }
}

/// What one refusal reason means, in the words the refusal prints.
fn reason_text(reason: &RefusalReason) -> &'static str {
    match reason {
        RefusalReason::Conflict(ConflictReason::LocalEdit { .. }) => "local edit",
        RefusalReason::Conflict(ConflictReason::MissingOwned { .. }) => "missing owned file",
        RefusalReason::Conflict(ConflictReason::UnexpectedFile { .. }) => "unexpected file",
        RefusalReason::Conflict(ConflictReason::EarlierInstall { .. }) => {
            "installed by an earlier clio without a marker"
        }
        RefusalReason::Conflict(ConflictReason::UnreadableMarker { .. })
        | RefusalReason::AbsentMarker { .. } => "unreadable marker",
        RefusalReason::ForeignInstaller { .. } => "installed by",
        RefusalReason::PayloadDigest { .. } => "payload digest differs",
        RefusalReason::Symlink { .. } => "symbolic link",
    }
}
