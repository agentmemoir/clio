// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the one question that decides where Clio keeps
//! its sync key, asked only on a deployment where multi-host sync is in play.
//!
//! ## Owns
//! - The two options and the wording that tells them apart
//! - The flag that answers the question without asking it
//! - The preview line for the chosen store, rendered from the run's environment
//!
//! ## Does not own
//! - When the question is asked (see `setup_steps`, which reads `/sync/omit`)
//! - Recording the choice (see `sync_key` in the config crate)
//! - Generating the key itself (that is `clio sync keygen`)
//!
//! ## Boundary
//!
//! Changes to what the two options say should normally require changes here.
//! Changes to how the sync key is stored should not.
//!
//! Keep this module focused on one question.
//!
//! # Why the file store is recommended here too
//!
//! The sync key's purpose is to be shared with a second machine. A key in the OS
//! secret store suits one operator on one machine; a key file can be placed in a
//! secret manager or an encrypted volume, which is how a shared deployment gets
//! the bytes to the other node. The keystore is still offered, because on a
//! single machine it is the stronger store and an operator may prefer it.

use crate::setup_cli_flows::Ctx;
use crate::setup_prompt::Choice;
use crate::setup_render::Signal;
use crate::setup_step_ask::Outcome;
use crate::setup_steps::Answers;
use clio_config::KeySource;

/// The flag that answers this question without asking it.
pub(crate) const FLAG: &str = "sync-store";

/// The question, its two options, and where the answer is stored.
///
/// # Errors
/// The question could not be asked, the operator went back, or the flag named
/// something that is not a store.
pub(crate) fn ask(
    ctx: &mut Ctx<'_>,
    answers: &mut Answers,
    recorded: Option<KeySource>,
) -> Result<Outcome, Signal> {
    // A recorded choice is not re-asked and not re-answered, the same rule the
    // encryption-key question follows.
    if let Some(source) = recorded {
        answers.sync_key_source = Some(source);
        return Ok(Outcome::Done);
    }
    if let Some(raw) = ctx.flag(FLAG) {
        let Some(source) = KeySource::parse(raw) else {
            return Err(Signal::usage(format!(
                "--{FLAG} must be {}, got `{raw}`",
                KeySource::accepted()
            )));
        };
        answers.sync_key_source = Some(source);
        return Ok(Outcome::Done);
    }
    if !ctx.interactive() {
        // Nothing to ask and no flag to read, so the recommended store is used
        // and the preview says so.
        answers.sync_key_source = Some(KeySource::recommended());
        return Ok(Outcome::Done);
    }
    let options = [
        Choice::new(
            KeySource::File.as_str(),
            "An owner-only sync key file",
            "put it in a secret manager or an encrypted volume to share it with another node",
        ),
        Choice::new(
            KeySource::Keystore.as_str(),
            "The operating system's secret store",
            "stronger on one machine, but it cannot be copied to another node",
        ),
    ];
    let default = KeySource::recommended().as_str();
    match ctx.ask_choice(
        "Where should Clio keep the sync key it sends to a relay?",
        &options,
        default,
    )? {
        crate::setup_render::Reply::Back => Ok(Outcome::Back),
        crate::setup_render::Reply::Value(value) => {
            let Some(source) = KeySource::parse(&value) else {
                return Err(Signal::Refused(format!(
                    "`{value}` is not a key store Clio knows"
                )));
            };
            answers.sync_key_source = Some(source);
            Ok(Outcome::Done)
        }
    }
}

/// The one line the preview shows for this answer.
///
/// `env` is the run's own environment, the same one `apply` records the choice
/// through, so the path printed here is the path that run will use. Reading the
/// process environment instead would print a path resolved from ambient state the
/// run never consults, and a test could not assert the two agree.
///
/// The wording names the recorded store choice and points at `clio sync keygen`
/// for the file itself: this wizard records where the key will live and does not
/// create it, so a preview that says the file will be there is promising
/// something the run does not do. The block above it, for the encryption key, is
/// different because that key does get created.
pub(crate) fn summary(source: KeySource, env: &dyn Fn(&str) -> Option<String>) -> String {
    match source {
        KeySource::File => match clio_config::sync_key::key_file_path(&|key: &str| env(key)) {
            Some(path) => format!(
                "the file store; run `clio sync keygen` to create an owner-only key at `{}` and \
                 share it through a secret manager or an encrypted volume",
                path.display()
            ),
            None => "the file store; run `clio sync keygen` to create the key (no path resolved)"
                .to_owned(),
        },
        KeySource::Keystore => {
            "the operating system's secret store (suits one operator on one machine); run \
             `clio sync keygen` to create the key"
                .to_owned()
        }
    }
}

#[cfg(test)]
#[path = "setup_sync_key_source_tests.rs"]
mod tests;
