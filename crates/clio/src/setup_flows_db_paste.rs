// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the half of the existing-Postgres route where
//! the operator supplies a whole address: the paste-or-type hand-over, the paste
//! route itself, and the SSL mode a pasted address asks for.
//!
//! ## Owns
//! - `ask_handover`, the paste-or-type sub-choice and its privacy difference
//! - `ask_pasted`, one address kept exactly as given
//! - `pasted_ssl`, reading the mode out of that address rather than overwriting it
//!
//! ## Does not own
//! - Which of the three databases the operator wants (see `setup_flows_db`)
//! - The dispatch between the two existing routes, including the hand-over's back
//!   landing on the sub-choice (see `setup_flows_db`)
//! - The typed route's fields and the blame step-back (see `setup_flows_db_typed`)
//! - Whether an address is valid (see `setup_validate`)
//!
//! ## Boundary
//!
//! Changes to how a pasted address is read or displayed should normally require
//! changes here. Changes to which route is chosen should not require changes here.
//!
//! Keep this module focused on the one address the operator hands over whole.

use crate::setup_cli_flows::Ctx;
use crate::setup_flows_db::{DB_ATTEMPTS, DatabaseAnswers, finish};
use crate::setup_plan_db::HandOver;
use crate::setup_prompt::Choice;
use crate::setup_render::{Reply, Signal};
use clio_store::SslChoice;

/// The paste-or-type choice, with the privacy difference stated at the point of
/// the choice rather than left for the operator to assume.
///
/// # Errors
/// The stream failed, the operator went back, or `--database-connect`
/// named something that is not a route. An answer typed at the question cannot be
/// one: the question refuses anything outside its own two options and re-asks.
pub(crate) fn ask_handover(ctx: &mut Ctx<'_>) -> Result<HandOver, Signal> {
    let options: [Choice; 2] = [
        Choice::new(
            "pasted",
            HandOver::Pasted.label(),
            "One line you paste, hidden as you type it. Clio reads it as given.",
        ),
        Choice::new(
            "typed",
            HandOver::Typed.label(),
            "Clio asks for each field, and checks each one as you go.",
        ),
    ];
    let default = match ctx.flag("database-connect") {
        Some(raw) => {
            // A flag is the operator speaking last, so it answers the question
            // rather than setting its default. The other two choices do the same,
            // and asking again here would consume an answer the caller thought it
            // had already given.
            return HandOver::parse(raw).ok_or_else(|| {
                Signal::usage(format!(
                    "--database-connect must be pasted|typed, got `{raw}`"
                ))
            });
        }
        None => match ctx.defaults().handover {
            Some(HandOver::Typed) => HandOver::Typed,
            _ => HandOver::Pasted,
        },
    };
    let default = match default {
        HandOver::Typed => "typed",
        HandOver::Pasted => "pasted",
    };
    if !ctx.interactive() {
        return Ok(HandOver::parse(default).expect("one of the two literals above"));
    }
    ctx.section("Postgres");
    ctx.warn(
        "Both routes hide what you type. A pasted string is one line and is taken as given; typed \
         fields are checked one at a time, so a mistake is caught before anything is written.",
    );
    let reply = ctx.ask_choice(
        "How would you like to give me the connection?",
        &options,
        default,
    )?;
    // The question resolved an empty line to the default and refused anything that
    // is neither route, so the answer is one of the two or the run has already
    // stopped. See `ask_choice_of_three` for why there is no refusal arm.
    match reply {
        Reply::Back => Err(Signal::Back),
        Reply::Value(value) => Ok(HandOver::parse(&value).expect("one of the two options")),
    }
}

/// The paste route: one address, kept exactly as given.
///
/// A paste cannot be masked, and pretending otherwise would be the one lie in
/// this wizard, so it is shown as it is entered. The address keeps its query
/// string because most cloud providers put `?sslmode=verify-full` in there and
/// rebuilding it would break the connection.
pub(crate) fn ask_pasted(ctx: &mut Ctx<'_>) -> Result<DatabaseAnswers, Signal> {
    ctx.section("Postgres address");
    let flag_value = ctx.flag("database-url").map(str::to_owned);
    let default = ctx.defaults().address.clone();
    for attempt in 1..=DB_ATTEMPTS {
        let address = match flag_value.clone() {
            Some(flagged) => flagged,
            None if !ctx.interactive() => {
                let stored = default.clone().unwrap_or_default();
                if stored.is_empty() {
                    return Err(Signal::usage(
                        "--database-url is required without a terminal".to_owned(),
                    ));
                }
                stored
            }
            // Read as a secret, because a connection string carries the password
            // in its userinfo part. Shown as it is typed, it would sit in the
            // terminal's scrollback and in every screenshot and transcript taken
            // afterwards, which is the one place a pasted secret always ends up.
            None => {
                let typed = ctx.ask_password("Connection string")?;
                if typed.trim().is_empty() {
                    default.clone().unwrap_or_default()
                } else {
                    typed
                }
            }
        };
        match crate::setup_validate::check_pasted_connection(&address) {
            Ok(()) => {
                let ssl = pasted_ssl(&address);
                let plan = crate::setup_plan_db::existing(address, ssl, Vec::new(), None)
                    .map_err(Signal::Refused)?;
                return finish(ctx, plan);
            }
            Err(reason) => {
                ctx.error(&reason);
                if attempt == DB_ATTEMPTS {
                    return Err(Signal::Refused(format!(
                        "the address was refused {DB_ATTEMPTS} times and nothing was written. The \
                         last reason was: {reason}"
                    )));
                }
            }
        }
    }
    // The loop returns on its last attempt, so this is unreachable rather than a
    // real outcome; a silent success here would be the worse bug.
    Err(Signal::Refused("the address could not be read".to_owned()))
}

/// The SSL mode a pasted address asked for, or the driver's default.
///
/// The address is kept exactly as given, so its mode is read rather than
/// overwritten. One read, through the store's own parser, so a pasted
/// `?sslmode=verify-full` keeps `verify-full`.
pub(crate) fn pasted_ssl(address: &str) -> SslChoice {
    clio_store::parse_connection(address).map_or(SslChoice::Prefer, |parsed| parsed.ssl)
}

#[cfg(test)]
#[path = "setup_flows_db_paste_tests.rs"]
mod tests;
