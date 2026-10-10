// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the extraction question: whether extraction
//! runs locally, is hosted, or is skipped, and the notice that hosted extraction
//! does not exist yet.
//!
//! ## Owns
//! - `ask_extract`, its flag decoding, and its three options
//! - The not-available notice printed when hosted extraction is chosen
//!
//! ## Does not own
//! - The step list (see `setup_steps`) or the other questions
//!   (see `setup_step_ask`)
//! - The extraction service itself (see `setup_services`)
//!
//! ## Boundary
//!
//! Changes to the extraction question should normally require changes here.
//! Changes to how a service is configured should not.
//!
//! Keep this module focused on the extraction question.
//!
//! # Why this is its own module
//!
//! `setup_step_ask` is at the workspace file-size limit and the sync-key
//! question must land beside it. The extraction question is the cohesive last
//! block of that file: it asks one question the other groups do not share.

use crate::setup_cli_flows::Ctx;
use crate::setup_prompt::Choice;
use crate::setup_render::{Reply, Signal};
use crate::setup_services::ServiceRole;
use crate::setup_step_ask::Outcome;
use crate::setup_steps::Answers;

/// The extraction question, whose hosted option records a skip.
///
/// Hosted extraction is not available yet. Choosing it prints the notice and
/// records a skip rather than configuring something that does not exist, which
/// is the honest outcome: a preview promising a hosted extractor would be a lie.
pub(crate) fn ask_extract(ctx: &mut Ctx<'_>, answers: &mut Answers) -> Result<Outcome, Signal> {
    if let Some(raw) = ctx.flag("extract") {
        let role = match raw.trim().to_ascii_lowercase().as_str() {
            "local" => ServiceRole::Local,
            // Hosted extraction does not exist yet, so it is recorded as a skip
            // rather than configuring something that is not there.
            "skip" | "none" | "online" | "hosted" => ServiceRole::Skip,
            other => {
                return Err(Signal::usage(format!(
                    "--extract must be local|skip|online, got `{other}`"
                )));
            }
        };
        if raw.eq_ignore_ascii_case("online") || raw.eq_ignore_ascii_case("hosted") {
            announce_extract_unavailable(ctx);
        }
        answers.extract = Some(role);
        return Ok(Outcome::Done);
    }
    if !ctx.interactive() {
        // The plan's flag default is skip, so a non-interactive run with no
        // `--extract` needs no question and no error.
        answers.extract = Some(ServiceRole::Skip);
        return Ok(Outcome::Done);
    }
    let options: [Choice; 3] = [
        Choice::new(
            "local",
            "Local Docker",
            "Pulls out facts and subjects from your notes, on this machine.",
        ),
        Choice::new(
            "hosted",
            "Hosted (not available yet)",
            "Not built yet. Choosing it records the same as Skip.",
        ),
        Choice::new(
            "skip",
            "Skip",
            "Turns extraction off. Clio still stores what you tell it directly.",
        ),
    ];
    let default = match answers.extract {
        Some(ServiceRole::Local) => "local",
        _ => "skip",
    };
    match ctx.ask_choice_aliased("Where should extraction come from?", &options, default)? {
        Reply::Back => Ok(Outcome::Back),
        Reply::Value(value) => {
            answers.extract = Some(match value.as_str() {
                "local" => ServiceRole::Local,
                "hosted" => {
                    announce_extract_unavailable(ctx);
                    ServiceRole::Skip
                }
                _ => ServiceRole::Skip,
            });
            Ok(Outcome::Done)
        }
    }
}

/// Print the not-available notice.
fn announce_extract_unavailable(ctx: &mut Ctx<'_>) {
    ctx.warn(
        "hosted extraction is not available yet; extraction is skipped (use --extract local to \
         run it through Docker)",
    );
}

#[cfg(test)]
#[path = "setup_step_extract_tests.rs"]
mod tests;
