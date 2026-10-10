// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the small field-level rules the database
//! questions share: what a usable username looks like, which question a failed
//! connection test sends the operator back to, and how an SSL mode is spelled
//! in an address.
//!
//! ## Owns
//! - The username rule, kept apart from the message that reports it
//! - The `Blame` label for each field a failed test can be sent back to
//! - The SSL mode names and the query fragment that carries one
//!
//! ## Does not own
//! - The checks themselves (see `setup_validate`)
//! - Which mode is the default (see `setup_validate::default_ssl_for`)
//! - Assembling an address (see `setup_plan_db`)
//!
//! ## Boundary
//!
//! Changes to the field rules should normally require changes here. Changes to
//! the checks or to address assembly should not require changes here.
//!
//! Keep this module focused on naming fields and their rules.

use crate::setup_validate::Blame;
use clio_store::SslChoice;

/// True when a username can go into an address unencoded.
///
/// The address puts the username before the password, so `:`, `@`, `/`, and
/// `#` would each end the username early or start the password early and
/// produce an address that connects as somebody else. Refusing them at the
/// question is the alternative to silently percent-encoding a value the
/// operator typed, which they would not recognise on the preview.
#[must_use]
pub(crate) fn username_ok(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty()
        && !value
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, ':' | '@' | '/' | '#' | '?' | '%'))
}

impl Blame {
    /// The question a failed test sends the operator back to.
    ///
    /// `Everything` gets no field, because a failure the driver did not
    /// attribute to anything in particular has no one answer to change. The
    /// caller offers the whole database section instead.
    #[must_use]
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Host => "Host",
            Self::Port => "Port",
            Self::Credentials => "Username or password",
            Self::Name => "Database name",
            Self::Ssl => "SSL mode",
            Self::Everything => "the database connection",
        }
    }
}

/// The query parameter that carries the SSL mode.
pub(crate) const SSL_PARAM: &str = "sslmode";

/// The query parameter that carries a caller-supplied root certificate.
pub(crate) const SSL_ROOT_PARAM: &str = "sslrootcert";

/// The modes, in the order the question offers them.
///
/// `verify-full` replaces the old `require`, and `encrypt-no-verify` is the one
/// deliberate way to encrypt without checking the server. `require` is not
/// offered: other Postgres clients use it for that no-check behaviour, and a
/// name that sounds safe while skipping the check is what this list avoids.
#[must_use]
pub(crate) fn ssl_mode_names() -> [&'static str; 4] {
    ["disable", "prefer", "verify-full", "encrypt-no-verify"]
}

/// The query fragment for one mode, e.g. `sslmode=verify-full`.
#[must_use]
pub(crate) fn ssl_query(mode: SslChoice) -> String {
    format!("{SSL_PARAM}={}", mode.as_str())
}

#[cfg(test)]
#[path = "setup_db_fields_tests.rs"]
mod tests;
