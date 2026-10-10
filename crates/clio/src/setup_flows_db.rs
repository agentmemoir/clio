// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the three-way database question and the two
//! routes that need nothing from the operator: Clio-managed Postgres and a
//! SQLite file. It also owns the hand-over sub-choice that selects between the
//! two existing-Postgres routes, and the fields both of them share.
//!
//! ## Owns
//! - `ask_database`, the whole database group
//! - The three choices and their defaults
//! - The paste-or-type sub-choice and its privacy warning
//! - The paste route and the connection test that closes it
//!
//! ## Does not own
//! - The typed route's six fields, the advanced block, and the blame step-back
//!   (see `setup_flows_db_typed`)
//! - The shared field reader and SSL question (see `setup_flows_db_fields`)
//! - The field checks (see `setup_validate`)
//! - Address assembly and the config entries (see `setup_plan_db`)
//!
//! ## Boundary
//!
//! Changes to which database questions are asked should normally require changes
//! here. Changes to what counts as a valid address should not require changes
//! here.
//!
//! Keep this module focused on collecting the database answers.
//!
//! # Why the typed route has its own module
//!
//! Six fields, thirteen advanced settings, a per-field blame step-back, and the
//! attempt cap is more than one file's worth. Splitting on the route boundary —
//! "the operator supplied nothing" against "the operator supplied everything" —
//! keeps each file about one shape of answer rather than one question.
//!
//! # Why a back from a hand-over field returns to the sub-choice
//!
//! The plan specifies that on the paste-or-type question, a back from the first
//! detail field goes back to the choice. The sub-choice is one keystroke further
//! away than that, and re-asking the sub-choice is the least surprising place to
//! land, so both routes catch their own back and return to it. A back from the
//! sub-choice itself has no earlier question inside the group, so it is handed to
//! `ask_database`, which re-asks the three-way question rather than letting it
//! reach the step loop.

use crate::setup_cli_flows::Ctx;
use crate::setup_flows_db_paste::ask_handover;
use crate::setup_plan_db::{DatabaseChoice, DatabasePlan, HandOver};
use crate::setup_prompt::Choice;
use crate::setup_render::{Reply, Signal};
use crate::setup_validate::{check_database_name, check_host, check_port, check_username};
use clio_store::SslChoice;
use std::path::PathBuf;

pub(crate) use crate::setup_flows_db_paste::ask_pasted;
pub(crate) use crate::setup_flows_db_typed::ask_typed;

/// What one pass through the database questions produced.
///
/// Deliberately not `PartialEq`: a test asserts the `entries()` two routes
/// produce, which is the thing that gets written, rather than comparing two
/// structs field by field. That keeps the assertion on the contract and not on
/// the shape of this type.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DatabaseAnswers {
    /// The finished plan, once the connection test passed.
    pub(crate) plan: DatabasePlan,
    /// What the probe reported, or `None` when nothing was probed.
    pub(crate) tested: Option<String>,
}

/// How many times one connection test is retried before the wizard gives up.
///
/// The plan's number. Three is enough to fix a typo and few enough that a
/// genuinely unreachable address does not turn into an interrogation.
pub(crate) const DB_ATTEMPTS: usize = 3;

/// Ask the whole database group and return the finished plan.
///
/// A `Signal::Back` from the three-way question is passed straight up, so a back
/// on the first question of the wizard exits. A managed or a SQLite run has no
/// probe to run: there is no address to open, and the plan says so rather than
/// inventing one.
///
/// # Errors
/// The operator quit, input ran out, or the connection test failed its attempts.
pub(crate) fn ask_database(ctx: &mut Ctx<'_>) -> Result<DatabaseAnswers, Signal> {
    // Whether the three-way question offers `Quit` rather than `Go back`, which
    // is true exactly when nothing is behind it: on the first pass, and again if
    // a back from a sub-route brings the operator back here with nothing
    // answered. Captured before the question, because answering it clears the
    // flag for every question after it.
    let at_run_start = ctx.back_leaves_run();
    loop {
        let choice = ask_choice_of_three(ctx)?;
        // The advanced settings belong to the typed route, because that is the only
        // route where the operator is typing the address. Accepting the flag on the
        // other two would be a silent no-op: the run would report success and the
        // setting would never reach the address, which is worse than refusing.
        if choice != DatabaseChoice::Existing && ctx.has_flag("database-option") {
            return Err(Signal::usage(
                "--database-option applies to `--database existing` with \
                 `--database-connect typed`, which is the only route that asks for it",
            ));
        }
        if choice != DatabaseChoice::Existing && ctx.has_flag("database-ssl-rootcert") {
            return Err(Signal::usage(
                "--database-ssl-rootcert applies to `--database existing`, the only route that \
                 reaches a server whose certificate Clio did not issue",
            ));
        }
        // A sub-route has questions of its own, so a back from inside one lands
        // here rather than leaving the wizard. The hand-over question is the first
        // of those, so a back from it asks this question again instead of
        // reaching the step loop, where a back on the first step would leave.
        let back = match choice {
            DatabaseChoice::Managed => Ok(DatabaseAnswers {
                plan: ask_managed(ctx)?,
                tested: None,
            }),
            DatabaseChoice::Sqlite => Ok(DatabaseAnswers {
                plan: ask_sqlite(ctx)?,
                tested: None,
            }),
            DatabaseChoice::Existing => ask_existing(ctx),
        };
        match back {
            Ok(answers) => return Ok(answers),
            Err(Signal::Back) => ctx.set_back_leaves_run(at_run_start),
            Err(other) => return Err(other),
        }
    }
}

/// The three-way question. Managed is the default on a fresh install.
///
/// # Errors
/// The stream failed, the operator went back, or `--database` named
/// something that is not a choice. An answer typed at the question cannot be one:
/// the question refuses anything outside its own three options and re-asks.
fn ask_choice_of_three(ctx: &mut Ctx<'_>) -> Result<DatabaseChoice, Signal> {
    let managed = managed_choice(ctx);
    let options = [
        managed,
        crate::setup_prompt::Choice::new(
            "sqlite",
            DatabaseChoice::Sqlite.label(),
            "One file on this machine. No Docker at all.",
        ),
        crate::setup_prompt::Choice::new(
            "existing",
            DatabaseChoice::Existing.label(),
            "You give Clio an address. Use this for a cloud database.",
        ),
    ];
    // A flag is the operator speaking last, so it beats a stored default.
    if let Some(raw) = ctx.flag("database") {
        let choice = DatabaseChoice::parse(raw).ok_or_else(|| {
            Signal::usage(format!(
                "--database must be managed|sqlite|existing, got `{raw}`"
            ))
        })?;
        refuse_managed_without_docker(ctx, choice).map_err(Signal::Refused)?;
        return Ok(choice);
    }
    // SQLite is the fallback for everything the two rules below do not claim: a
    // stored managed answer on a machine with no Docker, and no stored answer at
    // all on one. A stored `existing` answer is the only other one kept verbatim,
    // because it needs nothing from the machine either.
    //
    // A stored managed answer is only the default when Docker can still run it: a
    // machine that had Docker last week and does not today must not be walked into
    // the same failure by pressing Enter.
    let default = match ctx.defaults().database {
        Some(DatabaseChoice::Managed) | None if ctx.docker().is_available() => "managed",
        Some(DatabaseChoice::Existing) => "existing",
        _ => "sqlite",
    };
    if !ctx.interactive() {
        // The plan's flag default is managed, matching the interactive default, so
        // a non-interactive run with no `--database` needs no error — unless Docker
        // cannot run it, in which case the default has already moved to sqlite.
        return Ok(DatabaseChoice::parse(default).expect("one of the three literals above"));
    }
    loop {
        // `ask_choice` resolves an empty line to the default and refuses anything
        // that is not one of the three options, so by the time the answer arrives it
        // is one of them. The parse below therefore cannot fail, and an arm no
        // answer can reach would be a refusal about a bug rather than about anything
        // the operator did — and one more thing that has to stay true.
        let value = match ctx.ask_choice("Where should your memories live?", &options, default)? {
            Reply::Back => return Err(Signal::Back),
            Reply::Value(value) => value,
        };
        // An unavailable option is refused here rather than in the prompt, so the
        // menu keeps its three numbered lines and the operator is told why on
        // re-ask instead of being offered a choice that cannot work.
        if let Err(why) = refuse_managed_without_docker(
            ctx,
            DatabaseChoice::parse(&value).unwrap_or(DatabaseChoice::Sqlite),
        ) {
            ctx.error(&why);
            continue;
        }
        return Ok(DatabaseChoice::parse(&value).expect("one of the three options"));
    }
}

/// The managed option, labelled with whether this machine can actually run it.
///
/// Docker has three states, not two: running, installed but stopped, and not
/// installed. All three are checked before the first question so the menu can
/// answer the only question the operator has about option 1, which is whether it
/// will work.
fn managed_choice(ctx: &Ctx<'_>) -> Choice {
    let choice = crate::setup_prompt::Choice::new(
        "managed",
        DatabaseChoice::Managed.label(),
        if ctx.docker().is_available() {
            "Clio runs Postgres for you in Docker. No setup, no password to remember."
        } else {
            "Not available on this machine right now."
        },
    );
    if ctx.docker().is_available() {
        choice.with_note("Docker is running")
    } else {
        choice.with_note("Docker is not running")
    }
}

/// Refuse the managed choice when Docker cannot run it, naming what to fix.
///
/// # Errors
/// The reason, in the operator's terms: whether Docker needs starting or needs
/// installing, and which two options remain.
fn refuse_managed_without_docker(ctx: &Ctx<'_>, choice: DatabaseChoice) -> Result<(), String> {
    if choice != DatabaseChoice::Managed || ctx.docker().is_available() {
        return Ok(());
    }
    Err(format!(
        "Docker is not available, so Clio cannot run its own Postgres: {}. Start Docker and run \
         this again, or choose a SQLite file or a Postgres you already have.",
        ctx.docker().reason()
    ))
}

/// The managed route: no questions at all.
///
/// Clio created the database, so every value that identifies it is Clio's to
/// choose, not the operator's. The user, the database name, and the host are
/// fixed by what the container will actually run. The only value with more than
/// one acceptable answer is the port, because another program may already hold
/// the one Clio prefers, so [`pick_managed_port`] walks the range down until it
/// finds a free one.
///
/// There is no password to choose, and none is generated. The managed database
/// uses the image's trust authentication and is published on loopback only, so
/// the port binding is the boundary.
///
/// Every field is still reachable as a flag, so a script that needs a specific
/// port has one and an interactive operator is never asked for any of it.
fn ask_managed(ctx: &mut Ctx<'_>) -> Result<DatabasePlan, Signal> {
    // A password for the managed database would be a credential for a server
    // that has none, and one that silently did nothing. Refuse it by name
    // rather than ignore it, the same way `--database-option` is refused on the
    // routes that cannot use it.
    if ctx.has_flag("database-password") {
        return Err(Signal::usage(
            "--database-password does not apply to `--database managed`: Clio's own Postgres has \
             no password and is published on this machine only. It is for \
             `--database existing --database-connect typed`.",
        ));
    }
    let host = match flagged_checked(ctx, "database-host", check_host)? {
        Some(flagged) => flagged,
        None => crate::setup_managed::MANAGED_HOST.to_owned(),
    };
    let port = match flagged_checked(ctx, "database-port", check_port)? {
        Some(flagged) => flagged,
        None => crate::setup_managed::pick_managed_port().map_err(Signal::Refused)?,
    };
    let user = match flagged_checked(ctx, "database-user", check_username)? {
        Some(flagged) => flagged,
        None => crate::setup_managed::MANAGED_USER.to_owned(),
    };
    let name = match flagged_checked(ctx, "database-name", check_database_name)? {
        Some(flagged) => flagged,
        None => crate::setup_managed::MANAGED_DATABASE.to_owned(),
    };
    // A container Clio just started has no certificate, so `prefer` is the only
    // mode that can succeed; asking would be asking a question with one answer.
    let ssl = match ctx.flag("database-ssl") {
        Some(raw) => SslChoice::parse(raw).map_err(Signal::usage)?,
        None => SslChoice::Prefer,
    };
    // The line names a value Clio picked, so it also has to name the whole
    // command that changes it. `--database-ssl` on its own is not a command
    // anybody can run, which is the one thing a note like that must never be.
    ctx.confirm(&format!(
        "Clio will run its own Postgres at {host}:{port}, database `{name}`, as user `{user}`, \
         with SSL mode {ssl_name}. It is published on this machine only and has no password.",
        ssl_name = ssl.as_str()
    ));
    ctx.warn(
        "Nothing to answer here. To change any of it, run `clio setup --database-port \
         34309` (or any port from 34300 to 34310), or add `--database-ssl disable|prefer`.",
    );
    crate::setup_plan_db::managed(&host, &port, &name, &user, ssl, None).map_err(Signal::Refused)
}

/// A checked value from a flag, or nothing when the flag was not given.
///
/// The managed route has no question behind any field, so a script is the only
/// way to change one. The check still runs, so a bad flag is refused by name
/// rather than reaching the address and failing at the first connection.
///
/// # Errors
/// The flag's value is outside what the field's rule accepts.
fn flagged_checked<C>(ctx: &Ctx<'_>, flag: &str, check: C) -> Result<Option<String>, Signal>
where
    C: FnOnce(&str) -> Result<(), String>,
{
    ctx.flag(flag)
        .map(|flagged| {
            check(flagged).map_err(|reason| Signal::usage(format!("--{flag} {reason}")))?;
            Ok(flagged.to_owned())
        })
        .transpose()
}

/// The SQLite route: a path, defaulted to the standard location.
fn ask_sqlite(ctx: &mut Ctx<'_>) -> Result<DatabasePlan, Signal> {
    let default = ctx
        .defaults()
        .sqlite_path
        .clone()
        .unwrap_or_else(|| ctx.standard_sqlite_path().to_path_buf());
    let default = default.to_string_lossy().into_owned();
    let path = if let Some(raw) = ctx.flag("db-path") {
        raw.to_owned()
    } else if !ctx.interactive() {
        default
    } else {
        ctx.ask_checked("SQLite file", Some(&default), |value| {
            if value.trim().is_empty() {
                Err("the file path is required".to_owned())
            } else {
                Ok(())
            }
        })?
    };
    Ok(crate::setup_plan_db::sqlite(PathBuf::from(path), None))
}

/// The existing route: paste a string, or type the six fields.
///
/// The loop is what makes a back from a hand-over field land here rather than
/// leaving the wizard: the plan puts the sub-choice one keystroke before the
/// fields, so a back in the fields returns to it instead of to the question
/// above.
///
/// The loop only runs when the sub-choice was actually asked. A
/// `--database-connect` flag answers it without a question, and looping then asks
/// the fields over and over with nothing for a back to land on and no way out
/// but killing the run.
///
/// # Errors
/// The operator quit, input ran out, or the connection test failed its attempts.
fn ask_existing(ctx: &mut Ctx<'_>) -> Result<DatabaseAnswers, Signal> {
    let asked = ctx.flag("database-connect").is_none();
    loop {
        let route = ask_handover(ctx)?;
        let outcome = match route {
            HandOver::Pasted => ask_pasted(ctx),
            HandOver::Typed => ask_typed(ctx),
        };
        if asked && matches!(outcome, Err(Signal::Back)) {
            continue;
        }
        return outcome;
    }
}

/// Run the connection test and close the plan.
///
/// A pasted address is refused here rather than re-asked, because a paste is a
/// whole address: there is no field inside it to send the operator back to, and
/// asking for the whole string again after a connection failure would offer them
/// nothing to change.
///
/// With `--allow-untested` a refusal is recorded as an untested address with the
/// plan's warning instead of stopping the run, which is the flag-driven
/// equivalent of the interactive "save anyway" offer.
///
/// # Errors
/// The probe refused with no `--allow-untested`, and nothing was written.
pub(crate) fn finish(ctx: &mut Ctx<'_>, plan: DatabasePlan) -> Result<DatabaseAnswers, Signal> {
    match ctx.test_database(&plan.address) {
        Ok(detail) => Ok(DatabaseAnswers {
            plan,
            tested: Some(detail),
        }),
        Err(_) if ctx.has_flag("allow-untested") => {
            ctx.warn(crate::setup_connect::UNTESTED_WARNING);
            Ok(DatabaseAnswers { plan, tested: None })
        }
        Err(reason) => Err(Signal::Refused(format!(
            "the connection to {} failed and nothing was written: {}",
            plan.describe(),
            reason.reason()
        ))),
    }
}

#[cfg(test)]
#[path = "setup_flows_db_tests.rs"]
mod tests;

/// The refusals and the no-terminal defaults, in their own module.
///
/// Split out because `setup_flows_db_tests.rs` is already at the workspace's
/// file-size limit and these scenarios are a different question from the ones
/// that reach a plan: they are the ones that end a run before it gets there.
#[cfg(test)]
#[path = "setup_flows_db_refusal_tests.rs"]
mod refusal_tests;

/// The field defaults and the thirteen advanced settings, in their own module.
///
/// Split out for the same reason: one connection's settings are a different
/// question from which route asks for them.
#[cfg(test)]
#[path = "setup_flows_db_settings_tests.rs"]
mod settings_tests;
