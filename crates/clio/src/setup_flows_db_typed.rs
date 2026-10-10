// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the typed route into an existing Postgres: the
//! six detail fields and the connection test that closes the group and steps
//! back to the field the server blamed.
//!
//! ## Owns
//! - `ask_typed`, the whole route including the attempt cap
//! - The six fields and their defaults
//! - `Field`, the step-back target a failed test chooses
//! - The advanced settings a `--database-option` flag names
//!
//! ## Does not own
//! - The three-way question, the hand-over sub-choice, and the paste route
//!   (see `setup_flows_db`)
//! - The field checks (see `setup_validate`) and the settings table
//!   (see `setup_db_options`)
//! - Address assembly (see `setup_plan_db`)
//!
//! ## Boundary
//!
//! Changes to the typed fields, the advanced block, or the blame step-back should
//! normally require changes here. Changes to the paste route or to what counts
//! as a valid address should not require changes here.
//!
//! Keep this module focused on the route where the operator types everything.
//!
//! # Why the fields are held rather than asked straight into the address
//!
//! A failed test has to send the operator back to one field, and that needs a
//! field index that only exists if the answers are kept. So they are collected
//! into [`Details`], and the address is assembled from that once the group is
//! complete. Re-entering after a failure clears the blamed field and the ones
//! after it, which is what makes "the port was wrong" cost one keystroke rather
//! than six.

use crate::setup_cli_flows::Ctx;
use crate::setup_flows_db::{DB_ATTEMPTS, DatabaseAnswers};
use crate::setup_flows_db_fields::{ask_ssl, flagged_or_asked};
use crate::setup_plan_db::Extra;
use crate::setup_render::Signal;
use crate::setup_validate::{
    Blame, check_advanced, check_database_name, check_host, check_port, check_username,
};
use clio_store::SslChoice;

/// The six answers, held so a failed test can clear one of them.
///
/// `None` means "not answered yet", which is distinct from an answered field
/// whose value is the empty string.
#[derive(Debug, Clone, Default)]
struct Details {
    host: Option<String>,
    port: Option<String>,
    name: Option<String>,
    user: Option<String>,
    password: Option<String>,
    ssl: Option<SslChoice>,
    /// The root certificate a verifying mode trusts, or empty for none.
    ssl_root: Option<String>,
}

/// Which field a failed test sends the operator back to, in field order.
///
/// The order is the order the questions are asked in, so `from_blame` returning
/// a field also says how much of the group has to be re-asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Field {
    Host,
    Port,
    Name,
    /// The username and the password together.
    ///
    /// A server that refuses a login does not say which half was wrong, and it
    /// cannot: telling the operator the password is at fault when the username
    /// was the typo sends them to fix the one thing that was right.
    Credentials,
    Ssl,
}

impl Field {
    /// The label the blame message uses.
    #[must_use]
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Host => "Host",
            Self::Port => "Port",
            Self::Name => "Database name",
            Self::Credentials => "Username or password",
            Self::Ssl => "SSL mode",
        }
    }

    /// The question, and so the default, a blame sends the operator back to.
    #[must_use]
    pub(crate) const fn from_blame(blame: Blame) -> Option<Self> {
        match blame {
            Blame::Host => Some(Self::Host),
            Blame::Port => Some(Self::Port),
            Blame::Credentials => Some(Self::Credentials),
            Blame::Name => Some(Self::Name),
            Blame::Ssl => Some(Self::Ssl),
            // Nothing in particular was blamed, so no single field is a better
            // place to start than the first one.
            Blame::Everything => None,
        }
    }

    /// Forget this field and everything asked after it.
    ///
    /// A field below the blamed one is kept as the default for its re-ask, so a
    /// wrong port does not cost the operator retyping the host.
    ///
    /// The advanced settings are never cleared. The server blamed one named field
    /// or it blamed nothing in particular; it never said anything about the other
    /// twelve, so re-asking them would make an operator confirm thirteen defaults
    /// again for no reason at all.
    fn clear_from(self, details: &mut Details) {
        match self {
            Self::Host => {
                details.host = None;
                details.port = None;
                details.name = None;
                details.user = None;
                details.password = None;
                details.ssl = None;
                details.ssl_root = None;
            }
            Self::Port => {
                details.port = None;
                details.name = None;
                details.user = None;
                details.password = None;
                details.ssl = None;
                details.ssl_root = None;
            }
            Self::Name => {
                details.name = None;
                details.user = None;
                details.password = None;
                details.ssl = None;
                details.ssl_root = None;
            }
            Self::Credentials => {
                details.user = None;
                details.password = None;
                details.ssl = None;
                details.ssl_root = None;
            }
            Self::Ssl => {
                details.ssl = None;
                details.ssl_root = None;
            }
        }
    }
}

/// The typed route: the six fields, the advanced block, then the test.
///
/// The test is inside this loop because a failure steps back to the blamed field
/// and re-enters with the other answers as defaults. Nothing is written
/// meanwhile, so re-entering costs the operator nothing.
///
/// # Errors
/// The operator quit, input ran out, or the connection test failed its attempts.
pub(crate) fn ask_typed(ctx: &mut Ctx<'_>) -> Result<DatabaseAnswers, Signal> {
    let mut details = Details::default();
    let mut extra: Vec<Extra> = Vec::new();
    for attempt in 1..=DB_ATTEMPTS {
        ctx.section("Postgres details");
        ask_the_fields(ctx, &mut details, &mut extra)?;
        let root = details
            .ssl_root
            .as_deref()
            .filter(|path| !path.trim().is_empty());
        let address = crate::setup_plan_db::typed_address(
            field(&details.host),
            field(&details.port),
            field(&details.name),
            field(&details.user),
            field(&details.password),
            details.ssl.unwrap_or(SslChoice::Prefer),
            root,
            &extra,
        )
        .map_err(Signal::Refused)?;
        let ssl = details.ssl.unwrap_or(SslChoice::Prefer);
        // The plan records the extras it was built from, and the loop re-enters
        // with them, so the copy is what keeps a re-ask from re-asking the block.
        #[allow(
            clippy::redundant_clone,
            reason = "the loop re-enters with these, and the address borrows them"
        )]
        let plan = crate::setup_plan_db::existing(address, ssl, extra.clone(), None)
            .map_err(Signal::Refused)?;
        match ctx.test_database(&plan.address) {
            Ok(detail) => {
                return Ok(DatabaseAnswers {
                    plan,
                    tested: Some(detail),
                });
            }
            // A waived test records nothing and keeps the plan, which is the
            // flag-driven equivalent of the interactive "save anyway". It is
            // decided before the blame step-back, because re-asking a field is
            // the opposite of what the operator just asked for.
            Err(_) if ctx.has_flag("allow-untested") => {
                ctx.warn(crate::setup_connect::UNTESTED_WARNING);
                return Ok(DatabaseAnswers { plan, tested: None });
            }
            Err(reason) => {
                let blame = Blame::of(reason.reason());
                let blamed = Field::from_blame(blame);
                let named = blamed.map_or_else(|| blame.label(), Field::label);
                if let Some(field) = blamed {
                    field.clear_from(&mut details);
                }
                ctx.error(reason.reason());
                ctx.warn(&format!(
                    "The server blamed {named}. That question will be asked again with your \
                     other answers as defaults."
                ));
                if attempt == DB_ATTEMPTS {
                    return Err(Signal::Refused(format!(
                        "the connection failed {DB_ATTEMPTS} times, blaming {named}, and nothing \
                         was written. The last reason was: {}",
                        reason.reason()
                    )));
                }
            }
        }
    }
    // Unreachable: the loop returns on its last attempt. Named rather than left
    // implicit, because an empty success here would write nothing and say it did.
    Err(Signal::Refused(
        "the database answers could not be completed".to_owned(),
    ))
}

/// Ask whichever fields are still empty, in the plan's order.
///
/// Skipping the fields that already hold an answer is what makes the blame
/// step-back cost one question rather than six, and it is also what makes
/// `--database-host` reach a run whose other fields come from flags.
fn ask_the_fields(
    ctx: &mut Ctx<'_>,
    details: &mut Details,
    extra: &mut Vec<Extra>,
) -> Result<(), Signal> {
    if details.host.is_none() {
        let default = ctx.defaults().host.clone();
        details.host = Some(flagged_or_asked(
            ctx,
            "database-host",
            "Host",
            default,
            check_host,
        )?);
    }
    if details.port.is_none() {
        let default = Some(
            ctx.defaults()
                .port
                .clone()
                .unwrap_or_else(|| "5432".to_owned()),
        );
        details.port = Some(flagged_or_asked(
            ctx,
            "database-port",
            "Port",
            default,
            check_port,
        )?);
    }
    if details.user.is_none() {
        let default = ctx.defaults().user.clone();
        details.user = Some(flagged_or_asked(
            ctx,
            "database-user",
            "Username",
            default,
            check_username,
        )?);
    }
    if details.name.is_none() {
        // The default is `clio`, never the username. A default that mirrors the
        // answer to the previous question is accepted without being read, which
        // is how a database ends up named after a mistyped user.
        let default = Some(
            ctx.defaults()
                .name
                .clone()
                .unwrap_or_else(|| crate::setup_managed::MANAGED_DATABASE.to_owned()),
        );
        details.name = Some(flagged_or_asked(
            ctx,
            "database-name",
            "Database name",
            default,
            check_database_name,
        )?);
    }
    if details.password.is_none() {
        details.password = Some(match ctx.flag("database-password") {
            Some(flagged) => flagged.to_owned(),
            None if !ctx.interactive() => {
                return Err(Signal::usage(
                    "--database-password is required without a terminal".to_owned(),
                ));
            }
            None => ctx.ask_password("Password")?,
        });
    }
    if details.ssl.is_none() {
        details.ssl = Some(ask_ssl(ctx, field(&details.host))?);
    }
    if details.ssl_root.is_none() {
        // The root is a flag rather than a question, like the advanced settings:
        // it is only meaningful with a verifying mode, and most operators have
        // none to give. A pasted address can still carry `sslrootcert` itself.
        details.ssl_root = Some(
            ctx.flag("database-ssl-rootcert")
                .unwrap_or_default()
                .to_owned(),
        );
    }
    // The advanced settings are never asked. `--database-option` still sets
    // them, and the paste route still takes a whole address, so nothing an
    // expert needs became unreachable; what went away is thirteen questions
    // between the operator and a working install.
    if extra.is_empty() {
        *extra = flagged_advanced(ctx)?;
    }
    Ok(())
}

/// One held field, or an empty string.
///
/// Empty is refused by the address assembly rather than here, so a test can
/// build a `Details` without six assignments to reach the one field it cares
/// about.
// A held field, read as a string with an empty fallback.
//
// Taken by reference to the `Option` so the borrow outlives the call: the
// assembled address is built from these, and handing back a borrow of a
// temporary would not compile.
#[allow(clippy::ref_option, reason = "the borrow must outlive the call")]
fn field(slot: &Option<String>) -> &str {
    slot.as_deref().unwrap_or_default()
}

/// The advanced settings named by `--database-option name=value`.
///
/// One flag may appear more than once, so the map appends rather than overwrites:
/// a caller passing `--database-option options=-c%20k1 --database-option
/// options=-c%20k2` gets one setting with the last value, which is what a repeat
/// of a single-valued setting should mean.
///
/// # Errors
/// The pair has no `=`, or the name is not one of the thirteen, or the value is
/// outside what the driver accepts for it.
fn flagged_advanced(ctx: &Ctx<'_>) -> Result<Vec<Extra>, Signal> {
    let mut extra: Vec<Extra> = Vec::new();
    for raw in ctx.flag_values("database-option") {
        let (name, value) = raw.split_once('=').ok_or_else(|| {
            Signal::usage(format!(
                "--database-option needs name=value, got `{raw}` (for example \
                 --database-option application_name=clio)"
            ))
        })?;
        check_advanced(name, value)
            // The setting's own help and caveat ride along with the refusal, so
            // the one place an advanced setting is described is the one place a
            // mistyped one is reported.
            .map_err(|reason| {
                let setting = crate::setup_db_options::advanced(name).ok();
                let help = setting.map_or(String::new(), |s| format!(" ({})", s.help));
                let caveat = setting
                    .and_then(|s| s.caveat)
                    .map_or(String::new(), |c| format!(" {c}"));
                Signal::usage(format!("--database-option {reason}{help}{caveat}"))
            })?;
        let owned = value.to_owned();
        match extra.iter_mut().find(|entry| entry.name == name) {
            Some(entry) => entry.value.clone_from(&owned),
            None => extra.push(Extra {
                name: crate::setup_db_options::advanced(name)
                    .map_err(Signal::usage)?
                    .name,
                value: value.to_owned(),
            }),
        }
    }
    Ok(extra)
}
