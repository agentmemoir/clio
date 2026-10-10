// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the two field helpers both existing-Postgres
//! routes share: a value taken from its flag, a question, or the last run, and
//! the SSL-mode question whose default follows the host.
//!
//! ## Owns
//! - `flagged_or_asked`, the flag-or-question field reader
//! - `ask_ssl`, the SSL modes and the host-derived default
//!
//! ## Does not own
//! - The routes themselves (see `setup_flows_db` and `setup_flows_db_typed`)
//! - What a valid value is (see `setup_validate`)
//! - Address assembly (see `setup_plan_db`)
//!
//! ## Boundary
//!
//! Changes to how a shared database field is read should normally require
//! changes here. Changes to one route's questions should not.
//!
//! Keep this module focused on the field helpers both routes call.
//!
//! # Why these two live together
//!
//! They are the only pieces of the existing-Postgres path that both routes use
//! unchanged. Keeping them beside one route would make the other import across a
//! route boundary, and the next field helper would then be copied rather than
//! shared.

use crate::setup_cli_flows::Ctx;
use crate::setup_render::{Reply, Signal};
use clio_store::SslChoice;

/// One field from its flag, or from a question, or from the last run.
///
/// A flag is the operator speaking last, so it wins over both a stored default
/// and a question. Every field on this path has a flag, which is what makes a
/// non-interactive typed route possible.
///
/// # Errors
/// The stream failed, the operator went back, or no value satisfies the
/// field's rule.
pub(crate) fn flagged_or_asked<C>(
    ctx: &mut Ctx<'_>,
    flag: &str,
    label: &str,
    default: Option<String>,
    mut check: C,
) -> Result<String, Signal>
where
    C: FnMut(&str) -> Result<(), String> + 'static,
{
    if let Some(flagged) = ctx.flag(flag) {
        return check(flagged)
            .map(|()| flagged.to_owned())
            .map_err(|reason| Signal::usage(format!("--{flag} {reason}")));
    }
    if !ctx.interactive() {
        return default
            .ok_or_else(|| Signal::usage(format!("--{flag} is required without a terminal")));
    }
    ctx.ask_checked(label, default.as_deref(), check)
}

/// The SSL question, whose default follows the host.
///
/// A remote host defaults to `verify-full` because that is what providers expect
/// and because certificate checking is then not optional. This machine defaults
/// to `prefer`, so an unencrypted local server keeps working.
///
/// Choosing `encrypt-no-verify` prints, at the point of choice, what the mode
/// does and does not protect against. It is never a default and the operator has
/// to name it.
///
/// # Errors
/// The stream failed, the operator went back, or the answer named no
/// mode.
pub(crate) fn ask_ssl(ctx: &mut Ctx<'_>, host: &str) -> Result<SslChoice, Signal> {
    use crate::setup_prompt::Choice;
    let names = crate::setup_db_fields::ssl_mode_names();
    let options: [Choice; 4] = [
        Choice::new(
            names[0],
            "disable",
            "No encryption at all. Local servers only.",
        ),
        Choice::new(
            names[1],
            "prefer",
            "Encrypt and check the certificate when the server offers TLS, plain text when it \
             does not.",
        ),
        Choice::new(
            names[2],
            "verify-full",
            "Refuse to connect without encryption, and check the certificate and the server name. \
             Add sslrootcert=PATH for a private certificate authority.",
        ),
        Choice::new(
            names[3],
            "encrypt-no-verify",
            "Encrypt, but do not check the server certificate. Stops eavesdropping only.",
        ),
    ];
    let mode = if let Some(raw) = ctx.flag("database-ssl") {
        // A flag is the operator speaking last, so it beats a stored default and
        // the host-derived one.
        SslChoice::parse(raw).map_err(Signal::usage)?
    } else {
        let default = match ctx.defaults().ssl.as_deref() {
            Some(stored) if names.contains(&stored) => stored.to_owned(),
            _ => crate::setup_validate::default_ssl_for(host)
                .as_str()
                .to_owned(),
        };
        if ctx.interactive() {
            match ctx.ask_choice("SSL mode", &options, &default)? {
                Reply::Back => return Err(Signal::Back),
                Reply::Value(value) if value.trim().is_empty() => {
                    SslChoice::parse(&default).map_err(Signal::usage)?
                }
                Reply::Value(value) => SslChoice::parse(&value).map_err(Signal::usage)?,
            }
        } else {
            SslChoice::parse(&default).map_err(Signal::usage)?
        }
    };
    if mode == SslChoice::EncryptNoVerify {
        ctx.warn(
            "encrypt-no-verify encrypts the connection but does not check the server certificate. \
             It stops someone reading the traffic, but not someone who can intercept the \
             connection and pretend to be the server. Use verify-full with sslrootcert when you \
             hold the server's root certificate.",
        );
    }
    Ok(mode)
}
