// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the one question that decides where Clio keeps
//! its encryption key, and for the reason the two options differ.
//!
//! ## Owns
//! - The two options, the wording of each, and which one is recommended
//! - The flag that answers the question without asking it
//! - The preflight refusal for a machine that has already chosen
//!
//! ## Does not own
//! - Recording the choice (see `secret_key_source` in the config crate)
//! - Which questions come before or after this one (see `setup_steps`)
//! - The load path that reads whichever store was chosen (see `secret_key`)
//!
//! ## Boundary
//!
//! Changes to what the two options say, or to which one is recommended, should
//! normally require changes here. Changes to how the chosen store is read should
//! not.
//!
//! Keep this module focused on one question.
//!
//! # Why the file store is recommended
//!
//! It is the only store that works everywhere. A machine with no Secret Service, a
//! container, and a CI runner all have somewhere to put a file and nowhere to put
//! an OS secret, so recommending the keystore recommends a setup that a third of
//! the machines cannot complete.
//!
//! # Why the keystore is still offered, and described as the stronger one
//!
//! The keystore genuinely is stronger: the operating system protects it with the
//! same credentials that protect the rest of the user's secrets, and a key file is
//! only as private as the permissions on it. Saying so in the option's own hint is
//! how the operator learns it without the recommendation having to pretend
//! otherwise. Recommending the weaker option is a decision about what works, not a
//! claim about what is safer, and the hint is where that distinction is visible.

use crate::setup_cli_flows::Ctx;
use crate::setup_prompt::Choice;
use crate::setup_render::Signal;
use crate::setup_step_ask::Outcome;
use crate::setup_steps::Answers;
use clio_config::KeySource;

/// The flag that answers this question without asking it.
pub(crate) const FLAG: &str = "key-store";

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
    // A recorded choice is not re-asked and not re-answered. Reading it here is
    // what makes "once in the life of the install" true: the record is the
    // answer, and a second run adopts it rather than asking again.
    if let Some(source) = recorded {
        answers.key_source = Some(source);
        return Ok(Outcome::Done);
    }
    if let Some(raw) = ctx.flag(FLAG) {
        let source = KeySource::parse(raw).ok_or_else(|| {
            Signal::usage(format!(
                "--{FLAG} must be {}, got `{raw}`",
                KeySource::accepted()
            ))
        })?;
        answers.key_source = Some(source);
        return Ok(Outcome::Done);
    }
    if !ctx.interactive() {
        // Nothing to ask and no flag to read, so the recommended store is used and
        // the preview says so. Refusing here would make a non-interactive install
        // fail on a question the operator cannot see.
        answers.key_source = Some(KeySource::recommended());
        return Ok(Outcome::Done);
    }
    let options = [
        Choice::new(
            KeySource::File.as_str(),
            "An encrypted key file on this machine",
            "works everywhere, including a server or a container with no keychain",
        ),
        Choice::new(
            KeySource::Keystore.as_str(),
            "The operating system's secret store",
            "more secure: protected by your login credentials, not by file permissions",
        ),
    ];
    let default = KeySource::recommended().as_str();
    match ctx.ask_choice(
        "Where should Clio keep its encryption key?",
        &options,
        default,
    )? {
        crate::setup_render::Reply::Back => Ok(Outcome::Back),
        crate::setup_render::Reply::Value(value) => {
            let source = KeySource::parse(&value).ok_or_else(|| {
                Signal::Refused(format!("`{value}` is not a key store Clio knows"))
            })?;
            answers.key_source = Some(source);
            Ok(Outcome::Done)
        }
    }
}

/// The one line the preview shows for this answer.
///
/// The recommendation and the strength are both stated here, because the option
/// hints are gone by the time the operator reads the preview.
pub(crate) fn summary(source: KeySource) -> String {
    match source {
        KeySource::File => format!(
            "a key file at `{}` (recommended: it works on any machine)",
            clio_config::secret_key_file::key_file_path(&|key| std::env::var(key).ok())
                .map_or_else(
                    || "<no path resolved>".to_owned(),
                    |path| path.display().to_string()
                )
        ),
        KeySource::Keystore => "the operating system's secret store (more secure)".to_owned(),
    }
}
