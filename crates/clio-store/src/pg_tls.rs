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
//! - The connector choice for `disable` / `prefer` / `require`, including the
//!   fail-closed arm for a driver SSL mode this build has never seen
//! - Connection-string parsing through the driver's own parser
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
use postgres_rustls::MakeTlsConnector;
use rustls::{ClientConfig, RootCertStore};
use std::str::FromStr;
use std::sync::Arc;
use tokio_postgres::Config;
use tokio_postgres::config::{Host, SslMode};
use tokio_rustls::TlsConnector;

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

/// The SSL modes this store supports, named as `sslmode` names them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SslChoice {
    /// Connect in the clear whatever the server offers.
    Disable,
    /// Ask for TLS and carry on in the clear on the same socket if refused.
    #[default]
    Prefer,
    /// Require TLS, verified against the bundled public roots.
    Require,
}

impl SslChoice {
    /// The name `sslmode` uses for this mode.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Disable => "disable",
            Self::Prefer => "prefer",
            Self::Require => "require",
        }
    }

    /// Read a mode name as `sslmode` spells it.
    ///
    /// # Errors
    /// Any name outside the three. `verify-ca` and `verify-full` get their own
    /// message: the driver has no way to encrypt without checking the server, so
    /// there is nothing for those two to select, and "unsupported" on its own
    /// would leave the operator guessing what to type instead.
    pub fn parse(raw: &str) -> Result<Self, String> {
        let name = raw.trim().to_ascii_lowercase();
        match name.as_str() {
            "disable" => Ok(Self::Disable),
            "prefer" => Ok(Self::Prefer),
            "require" => Ok(Self::Require),
            "verify-ca" | "verify-full" => Err(format!(
                "sslmode `{raw}` cannot be honoured: this store always checks the server \
                 certificate against the bundled public roots, and the driver offers no way to \
                 encrypt without checking it. Use `require`, or `prefer` to fall back to plain \
                 text when the server refuses TLS"
            )),
            other => Err(format!(
                "sslmode must be disable|prefer|require, got `{other}`"
            )),
        }
    }

    /// Map a driver mode onto a choice, failing closed.
    ///
    /// `SslMode` is `#[non_exhaustive]`, so a mode added by a later driver
    /// release arrives here with no arm of its own. The `_` arm maps it to
    /// `Require`: a mode this build has never read must not be assumed to be the
    /// one that connects without encryption.
    #[must_use]
    pub fn of(mode: SslMode) -> Self {
        match mode {
            SslMode::Disable => Self::Disable,
            SslMode::Prefer => Self::Prefer,
            SslMode::Require => Self::Require,
            _ => Self::Require,
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
    let config = Config::from_str(url)
        .map_err(|e| format!("database url is not usable: {}", describe(&e)))?;
    Ok(Connection {
        hosts: config.get_hosts().iter().map(host_name).collect(),
        ports: config.get_ports().to_vec(),
        database: config.get_dbname().map(ToOwned::to_owned),
        user: config.get_user().map(ToOwned::to_owned),
        ssl: SslChoice::of(config.get_ssl_mode()),
    })
}

/// Open one connection, choosing the connector from the string's SSL mode.
///
/// - `disable` uses the plain connector.
/// - `require` uses the rustls connector with certificate checking on and the
///   bundled public roots, so `require` means a *verified* connection.
/// - `prefer` uses that same rustls connector. The driver sends `SSLRequest`,
///   reads the answer, and continues in the clear on the same socket when the
///   answer is no. There is no second attempt, so a wrong password produces one
///   failure rather than two.
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

/// The verified TLS connector: rustls, the bundled public roots, no client
/// certificate.
///
/// # Errors
/// rustls cannot build a client configuration from those roots. That set is a
/// build-time constant, so it does not happen in a working binary; it is still
/// reported rather than unwrapped, because a connector that quietly degraded to
/// no checking is the exact failure this stage exists to prevent.
fn rustls_connector() -> Result<MakeTlsConnector, Failure> {
    let config =
        ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .map_err(|e| Failure::settled(format!("tls setup: {e}")))?
            .with_root_certificates(RootCertStore {
                roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
            })
            .with_no_client_auth();
    Ok(MakeTlsConnector::new(TlsConnector::from(Arc::new(config))))
}

/// A connection or query that failed, with the reason split so that only a
/// refused socket is worth repeating.
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
    // Parse with the driver first: it owns what a connection string may say, and
    // its refusal is a better message than anything written here. The original
    // string is what the connection uses, so nothing read here can change the
    // address that is dialled.
    let config = Config::from_str(url)
        .map_err(|e| Failure::settled(format!("postgres url is not usable: {}", describe(&e))))?;
    let attempt = match SslChoice::of(config.get_ssl_mode()) {
        SslChoice::Disable => Client::connect(url, NoTls),
        SslChoice::Prefer | SslChoice::Require => Client::connect(url, rustls_connector()?),
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
