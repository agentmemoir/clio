// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the one decision about how a Postgres
//! connection is opened: the connector is chosen here from the requested SSL
//! mode, and every caller in the store goes through [`connect`].
//!
//! ## Owns
//! - The connector choice for every SSL mode this store offers
//! - Connection-string parsing, including the SSL options `pg_url` reads
//!   before the driver's own parser sees the string
//! - The local-or-remote answer for a connection string ([`Connection::is_local`])
//! - Whether a connection failure is worth repeating (`pg_probe` owns the
//!   window and reads that answer)
//!
//! ## Does not own
//! - Store bootstrap, schema, or the pgvector version gate (`postgres`)
//! - The setup-time probe itself (`pg_probe` owns it)
//! - Any HTTP endpoint rule (`clio-index` owns its transport, `clio-config`
//!   owns its own gate)
//!
//! ## Boundary
//!
//! Changes to how the store reaches a database should normally require changes
//! here. Changes to schema, queries, or the wizard's questions should not
//! require changes here.
//!
//! # Why there is one connector
//!
//! Callers used to pass `NoTls` each, so a cloud database that demands
//! encryption could not be reached at all, and one that does not demand it would
//! carry every memory in the clear. One function that picks the connector means
//! one place to get it right and one place to change it, and it is why every call
//! site — including the test helpers — goes through [`connect`].

use clio_types::{AmError, ErrorCode, is_loopback_host, url_scheme};
use postgres::{Client, NoTls};
use std::str::FromStr;
use tokio_postgres::Config;
use tokio_postgres::config::Host;

/// Port the driver falls back to when a connection string names none.
const DEFAULT_PORT: u16 = 5432;

/// The driver's own `Display` for a TLS handshake failure.
pub(crate) const TLS_HANDSHAKE_FAILED: &str = "error performing TLS handshake";

/// Driver `Display` strings for failures no later attempt can fix.
///
/// The driver exposes no predicate for these error kinds, so they are matched on
/// the fixed text it prints for them. The tests below pin two of these strings
/// against a real failure, so a driver release that changes one fails a test
/// rather than quietly turning a permanent failure into a 15-second wait.
const CLIENT_REFUSALS: &[&str] = &[
    TLS_HANDSHAKE_FAILED,
    // The client rejected the address before sending anything: a missing host,
    // a port count that does not match the host count.
    "invalid configuration",
    // An authentication method the driver cannot do, or a channel binding the
    // server would not provide. Both are properties of the pair, not of the
    // moment.
    "authentication error",
];

/// Driver `Display` strings that say only what kind of error happened.
///
/// When the chain below one of these says the reason, the reason is the useful
/// half and the kind is noise: an operator reading `db error` learns nothing
/// they can act on.
const PLACEHOLDER_REASONS: &[&str] = &[
    "db error",
    "error communicating with the server",
    TLS_HANDSHAKE_FAILED,
];

/// The SSL modes this store supports, named as `libpq` names them.
///
/// `require` is deliberately not offered. In `libpq` it means "encrypt and do
/// not check the server", and a name that sounds safe while skipping
/// verification is the footgun this list exists to remove. The two modes that
/// verify say so in their names, and the deliberate no-check mode says what it
/// is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SslChoice {
    /// Connect in the clear whatever the server offers.
    Disable,
    /// Ask for TLS and carry on in the clear on the same socket if refused.
    ///
    /// When the server does accept TLS the certificate is verified. This is
    /// stricter than `libpq`'s `prefer`, on purpose.
    #[default]
    Prefer,
    /// Require TLS, verified against the bundled roots plus any supplied root.
    VerifyFull,
    /// Require TLS without checking the server certificate.
    ///
    /// This protects against eavesdropping only. Anything that can intercept
    /// the connection can pretend to be the server, because nothing
    /// authenticates it. It is never a default.
    EncryptNoVerify,
}

impl SslChoice {
    /// The name `sslmode` uses for this mode.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Disable => "disable",
            Self::Prefer => "prefer",
            Self::VerifyFull => "verify-full",
            Self::EncryptNoVerify => "encrypt-no-verify",
        }
    }

    /// Read a mode name as `sslmode` spells it.
    ///
    /// Only the exact names are accepted: no name is a prefix of another, and a
    /// partial one is refused rather than resolved, so a typo cannot silently
    /// select a mode. `require`, `verify-ca`, and `allow` get their own message
    /// naming what to use instead, because an operator arriving from another
    /// Postgres client is likely to try them.
    ///
    /// # Errors
    /// Any name outside the four.
    pub fn parse(raw: &str) -> Result<Self, String> {
        let name = raw.trim().to_ascii_lowercase();
        match name.as_str() {
            "disable" => Ok(Self::Disable),
            "prefer" => Ok(Self::Prefer),
            "verify-full" => Ok(Self::VerifyFull),
            // Hard to type by mistake, and the only mode that skips the check.
            "encrypt-no-verify" => Ok(Self::EncryptNoVerify),
            "require" => Err(format!(
                "sslmode `{raw}` is not offered: other Postgres clients use `require` for \
                 encrypting without checking the server certificate. Use `verify-full` to check \
                 it, `encrypt-no-verify` to encrypt without checking it, or `prefer` to allow \
                 plain text when the server refuses TLS"
            )),
            "verify-ca" => Err(format!(
                "sslmode `{raw}` is not offered: this store checks the certificate chain and the \
                 server name together, so there is no certificate-authority-only mode. Use \
                 `verify-full`, and add `sslrootcert=<path>` to trust a private certificate \
                 authority"
            )),
            other => Err(format!(
                "sslmode must be disable|prefer|verify-full|encrypt-no-verify, got `{other}`"
            )),
        }
    }
}

/// What a connection string names, as far as setup needs to show and check it.
///
/// The fields exist for display, for defaults, and for the local-or-remote
/// question. The connection itself always uses the original string, so nothing
/// read here can change what that string connects to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connection {
    /// Hosts in the order the string listed them; empty when it named none.
    pub hosts: Vec<String>,
    /// Ports in the same order, with the driver's defaults filled in.
    pub ports: Vec<u16>,
    /// Database name, when the string named one.
    pub database: Option<String>,
    /// User, when the string named one.
    pub user: Option<String>,
    /// The SSL mode the string asked for.
    pub ssl: SslChoice,
}

impl Connection {
    /// Whether every host in the string is on this machine.
    ///
    /// A string that named no host is not local. The driver will refuse it, and
    /// telling an operator their undialable address is "on this machine" is
    /// worse than the opposite mistake.
    #[must_use]
    pub fn is_local(&self) -> bool {
        // A Unix socket path is on this machine by definition — there is no
        // network in it — so treating one as remote would put a "this connection
        // is not encrypted" warning in front of an operator already as local as
        // a connection can be.
        !self.hosts.is_empty()
            && self
                .hosts
                .iter()
                .all(|host| is_loopback_host(host) || host.starts_with('/'))
    }

    /// The first host, or an empty string when the string named none.
    #[must_use]
    pub fn primary_host(&self) -> &str {
        self.hosts.first().map_or("", String::as_str)
    }

    /// The first port, or the driver's default when the string named none.
    #[must_use]
    pub fn primary_port(&self) -> u16 {
        self.ports.first().copied().unwrap_or(DEFAULT_PORT)
    }
}

/// True when `host` is on this machine.
///
/// The one definition, re-exported from `clio-types` so this crate and the two
/// endpoint gates cannot drift apart into three answers.
#[must_use]
pub fn is_loopback(host: &str) -> bool {
    is_loopback_host(host)
}

/// The name to show for one of the driver's host entries.
fn host_name(host: &Host) -> String {
    // A Unix socket entry is a directory, and it is shown as it was written so
    // the operator recognises it. `Connection::is_local` reads it as local.
    match host {
        Host::Tcp(name) => name.clone(),
        Host::Unix(path) => path.to_string_lossy().into_owned(),
    }
}

/// Read a connection string with the driver's own parser.
///
/// The scheme is checked first, because the driver's parser does not check it:
/// anything that is not `postgres://` or `postgresql://` falls through to the
/// key/value form, so a `mysql://` string comes back as a complaint about an
/// unknown parameter rather than as a refusal naming what this store supports.
///
/// # Errors
/// The scheme is not `postgres` or `postgresql`, or the driver rejects the
/// string. A rejection from the driver is passed through with its own message.
pub fn parse_connection(url: &str) -> Result<Connection, String> {
    let scheme = url_scheme(url).ok_or_else(|| {
        format!("database url must start with postgres:// or postgresql://, got `{url}`")
    })?;
    if scheme != "postgres" && scheme != "postgresql" {
        return Err(format!(
            "database url scheme must be postgres:// or postgresql://, got `{scheme}://` \
             (this store keeps memories in PostgreSQL with pgvector; sqlite:// and mysql:// are \
             not database targets)"
        ));
    }
    let spec = crate::pg_url::spec(url)?;
    let config = Config::from_str(&spec.driver_url)
        .map_err(|e| format!("database url is not usable: {}", describe(&e)))?;
    Ok(Connection {
        hosts: config.get_hosts().iter().map(host_name).collect(),
        ports: config.get_ports().to_vec(),
        database: config.get_dbname().map(ToOwned::to_owned),
        user: config.get_user().map(ToOwned::to_owned),
        ssl: spec.ssl,
    })
}

/// Open one connection, choosing the connector from the string's SSL mode.
///
/// - `disable` uses the plain connector.
/// - `verify-full` requires TLS and checks the certificate chain and the server
///   name against the bundled public roots plus any `sslrootcert`.
/// - `prefer` uses that same verified connector. The driver sends `SSLRequest`,
///   reads the answer, and continues in the clear on the same socket when the
///   answer is no. There is no second attempt, so a wrong password produces one
///   failure rather than two.
/// - `encrypt-no-verify` requires TLS and checks nothing, which is the one
///   deliberate way to reach a server whose certificate cannot be verified.
///
/// The driver refuses `prefer` together with `sslnegotiation=direct` and says so
/// itself; that message is passed through rather than pre-empted, because the
/// driver is the one that knows the combination is meaningless.
///
/// # Errors
/// The string is not a connection string, the server cannot be reached, or it
/// refuses the connection or the certificate.
pub fn connect(url: &str) -> Result<Client, AmError> {
    open(url).map_err(Failure::am_error)
}

/// A connection or query that failed, with the reason split so that only a
/// refused socket is worth repeating.
#[derive(Debug)]
pub(crate) struct Failure {
    pub(crate) message: String,
    /// True when a later attempt could plausibly succeed.
    pub(crate) retryable: bool,
}

impl Failure {
    /// A failure a later attempt cannot fix: a bad address, or a refusal.
    pub(crate) fn settled(message: String) -> Self {
        Self {
            message,
            retryable: false,
        }
    }

    fn am_error(self) -> AmError {
        AmError::new(ErrorCode::Internal, self.message)
    }
}

/// Open a connection, keeping the driver's own error so a caller can tell a
/// refused socket from a refusal.
pub(crate) fn open(url: &str) -> Result<Client, Failure> {
    // Read the SSL options this store acts on first, then let the driver parse
    // the rewritten URL. The URL the connection uses is the one the driver can
    // accept, with `sslrootcert` taken out and `sslmode` mapped to a value it
    // knows; the connector alone decides whether the certificate is checked.
    let spec = crate::pg_url::spec(url).map_err(Failure::settled)?;
    if let Err(e) = Config::from_str(&spec.driver_url) {
        return Err(Failure::settled(format!(
            "postgres url is not usable: {}",
            describe(&e)
        )));
    }
    let attempt = match spec.ssl {
        SslChoice::Disable => Client::connect(&spec.driver_url, NoTls),
        SslChoice::Prefer | SslChoice::VerifyFull => Client::connect(
            &spec.driver_url,
            crate::tls_trust::verified_connector(spec.root.as_deref())?,
        ),
        SslChoice::EncryptNoVerify => {
            Client::connect(&spec.driver_url, crate::tls_trust::unverified_connector()?)
        }
    };
    attempt.map_err(|e| Failure {
        message: format!("postgres connect: {}", describe(&e)),
        retryable: is_retryable(&e),
    })
}

/// Whether a failed connection could succeed on a later attempt.
///
/// A server that is still starting refuses the socket before it can answer
/// anything. Once it does answer, the driver reports a database error, and the
/// same answer will come back on every later attempt; and the client-side
/// refusals in [`CLIENT_REFUSALS`] are properties of the address or the pair,
/// not of the moment.
fn is_retryable(err: &postgres::Error) -> bool {
    err.as_db_error().is_none() && !CLIENT_REFUSALS.contains(&err.to_string().as_str())
}

/// A driver error as text an operator can act on.
///
/// The driver's own `Display` for a server-side refusal is the bare string
/// `db error`, and the reason — `password authentication failed for user "x"` —
/// sits in the source. Walking the chain is the whole reason this function
/// exists, and dropping the placeholder half of it is what makes the reason read
/// as a reason.
pub(crate) fn describe(err: &postgres::Error) -> String {
    let mut chain: Vec<String> = vec![err.to_string()];
    let mut source = std::error::Error::source(err);
    while let Some(cause) = source {
        let text = cause.to_string();
        if chain.last() != Some(&text) {
            chain.push(text);
        }
        source = cause.source();
    }
    // Keep a placeholder only when nothing better follows it.
    let parts: Vec<&str> = chain
        .iter()
        .enumerate()
        .filter(|(index, text)| {
            index + 1 == chain.len() || !PLACEHOLDER_REASONS.contains(&text.as_str())
        })
        .map(|(_, text)| text.as_str())
        .collect();
    parts.join(": ")
}

#[cfg(test)]
#[path = "pg_tls_tests.rs"]
mod pg_tls_tests;

#[cfg(test)]
#[path = "pg_tls_db_tests.rs"]
mod pg_tls_db_tests;
