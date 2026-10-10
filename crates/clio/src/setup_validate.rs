// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for every format check `clio setup` runs before
//! it writes a database address or a service endpoint, and for routing a failed
//! connection test back to the field the server complained about.
//!
//! ## Owns
//! - Address checks: the driver's own parse, a host, and the SSL mode
//! - Endpoint checks: `http`/`https`, a host, and the remote plain-`http` rule
//! - Host, port, username, and database-name checks
//! - `default_ssl_for`, the mode a given host defaults to
//! - `Blame`, which turns a driver message into the field to re-ask
//!
//! ## Does not own
//! - The advanced-setting table (see `setup_db_options`)
//! - The field rules and their labels (see `setup_db_fields`)
//! - Assembling an address, and the connection test itself
//!
//! ## Boundary
//!
//! Changes to what counts as a usable address or endpoint should normally
//! require changes here. Changes to the driver's parser, to the loopback rule,
//! or to the HTTP transport should not require changes here.
//!
//! Keep this module focused on refusing a value, with no I/O.
//!
//! # Why the driver's parser and not a second one
//!
//! A hand-rolled check would disagree with the store about what can be opened,
//! and the wizard would then save an address the store refuses. So the
//! connection parse is `clio_store::parse_connection` itself, the loopback
//! question is `clio_types::net`, and nothing here re-derives either.

use crate::setup_db_fields::username_ok;
use crate::setup_db_options::advanced;
use clio_store::{SslChoice, is_loopback, parse_connection};

/// Which field a failed connection test should send the operator back to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Blame {
    /// The server could not be reached at that name.
    Host,
    /// Nothing is listening there, or it cannot be reached.
    Port,
    /// The server refused the username or the password.
    Credentials,
    /// The username was fine; the database name is not.
    Name,
    /// The server refused the encryption it was asked for.
    Ssl,
    /// The driver attributed it to nothing in particular.
    Everything,
}

impl Blame {
    /// Classify one probe failure message.
    ///
    /// Conservative by design: anything not matched is `Everything`, because
    /// sending an operator back to a field the server never mentioned sends
    /// them to change something that was already right.
    ///
    /// The driver's own `Display` for a server-side refusal is the bare string
    /// `db error`, and the reason sits in the error source below it. A caller
    /// that walks that chain and appends it gets a real classification; a caller
    /// that passes the bare string gets `Everything`, which is the honest answer
    /// for a message with nothing in it.
    #[must_use]
    pub(crate) fn of(message: &str) -> Self {
        let message = message.to_ascii_lowercase();
        let says = |needles: &[&str]| needles.iter().any(|n| message.contains(n));
        if says(&["certificate", "server refused", "ssl", "tls", "handshake"]) {
            return Self::Ssl;
        }
        if says(&[
            "password authentication failed",
            "authentication failed",
            "no password supplied",
            "authentication method",
            "sasl",
        ]) || (says(&["role"]) && says(&["does not exist"]))
        {
            return Self::Credentials;
        }
        if (says(&["database"]) && says(&["does not exist"]))
            || says(&["3d000", "invalid_catalog_name"])
        {
            return Self::Name;
        }
        if says(&[
            "name or service not known",
            "could not translate",
            "no such host",
        ]) {
            return Self::Host;
        }
        if says(&[
            "connection refused",
            "connection timed out",
            "timed out",
            "network is unreachable",
            "failed to connect",
            "no route to host",
        ]) {
            return Self::Port;
        }
        Self::Everything
    }
}

/// Check an assembled or pasted database address.
///
/// # Errors
/// The driver refuses the string, the string names no host, or it asks for an
/// SSL mode this store cannot honour.
pub(crate) fn check_connection(url: &str) -> Result<(), String> {
    // The SSL mode is read first, and through `clio-store`'s own reader rather
    // than the driver's, because that reader explains *why* `verify-full`
    // cannot be honoured. The driver refuses the same value with "invalid
    // value for option `sslmode`", which leaves an operator who typed it
    // guessing what to type instead.
    if let Some(mode) = ssl_mode_of(url) {
        SslChoice::parse(mode)?;
    }
    parse_connection(url)?;
    if clio_types::net::parse_scheme_host(url).is_err() {
        return Err(format!(
            "database address `{url}` names no host. Add the machine the server runs on, as in \
             `postgres://user:password@db.example.com:5432/clio`"
        ));
    }
    Ok(())
}

/// Check a pasted connection string.
///
/// # Errors
/// The string is not a complete `postgres://` or `postgresql://` URL, or
/// [`check_connection`] refuses it.
pub(crate) fn check_pasted_connection(url: &str) -> Result<(), String> {
    if !url.trim().starts_with("postgres://") && !url.trim().starts_with("postgresql://") {
        return Err(format!(
            "a pasted connection must be a complete address starting `postgres://` or \
             `postgresql://`, got `{url}`. The provider's own connection string is what belongs \
             here; a bare `host:port` is typed in field by field instead"
        ));
    }
    check_connection(url)
}

/// Check one host entry or a comma-separated host list.
///
/// # Errors
/// The host is empty, carries a scheme or userinfo, or holds whitespace.
pub(crate) fn check_host(host: &str) -> Result<(), String> {
    let host = host.trim();
    if host.is_empty() {
        return Err("host is required".to_owned());
    }
    if let Some(bad) = host.split(',').find(|part| part.trim().is_empty()) {
        return Err(format!(
            "host list `{host}` has an empty entry next to `{bad}`. A failover pair is one value \
             with a comma, such as `db1.example.com,db2.example.com`"
        ));
    }
    for part in host.split(',') {
        let part = part.trim();
        if part.chars().any(char::is_whitespace) {
            return Err(format!("host `{part}` contains a space"));
        }
        if part.contains("://") {
            return Err(format!(
                "host `{part}` must not include a scheme. Type the machine on its own, as in \
                 `db.example.com`"
            ));
        }
        if part.contains('@') {
            return Err(format!(
                "host `{part}` must not include a username. The username and password are their \
                 own fields"
            ));
        }
    }
    Ok(())
}

/// Check a port, or a comma-separated port list matching a host list.
///
/// # Errors
/// Any entry is not a number in 1..=65535.
pub(crate) fn check_port(port: &str) -> Result<(), String> {
    let port = port.trim();
    if port.is_empty() {
        return Err("port is required".to_owned());
    }
    for entry in port.split(',') {
        let entry = entry.trim();
        let ok = entry
            .parse::<u16>()
            .is_ok_and(|parsed| (1..=65535).contains(&parsed));
        if !ok {
            return Err(format!(
                "port `{entry}` must be a whole number between 1 and 65535"
            ));
        }
    }
    Ok(())
}

/// Check a username.
///
/// # Errors
/// Empty, or holding a character that would end the username inside the
/// address and connect as somebody else.
pub(crate) fn check_username(user: &str) -> Result<(), String> {
    if username_ok(user) {
        Ok(())
    } else {
        Err(format!(
            "username `{user}` cannot be used: it is empty or holds a character that would end \
             the username inside the address, such as `:`, `@`, `/`, or a space"
        ))
    }
}

/// Check a database name.
///
/// # Errors
/// Empty, holding whitespace, or holding a character the address reads as the
/// start of something else.
pub(crate) fn check_database_name(name: &str) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("database name is required".to_owned());
    }
    if let Some(bad) = name
        .chars()
        .find(|c| c.is_whitespace() || matches!(c, '/' | '?' | '#' | '@' | ':' | '%'))
    {
        return Err(format!(
            "database name `{name}` cannot hold `{bad}`. Use the name as the server has it, \
             which is letters, digits, `_`, `-`, and `$`"
        ));
    }
    Ok(())
}

/// Check a service endpoint.
///
/// This is the wizard's half of the encrypted-endpoint rule. The other half is
/// the request gate in `clio-index`, which refuses to send; the config gate in
/// `clio-config` covers `clio config set` and a hand-edited file. All three ask
/// the same loopback question, through one definition.
///
/// # Errors
/// The scheme is not `http` or `https`, there is no host, or a remote host is
/// named over plain `http`.
pub(crate) fn check_endpoint(url: &str) -> Result<(), String> {
    let target = clio_types::net::http_target(url)?;
    if target.scheme == "http" && !clio_types::net::is_loopback_host(&target.host) {
        let host = &target.host;
        return Err(format!(
            "endpoint `{url}` must use https for a remote host: plain http would send the API key \
             and every text in the clear. Change the scheme to `https://{host}`, or point it at a \
             service on this machine such as `http://127.0.0.1:<port>`"
        ));
    }
    Ok(())
}

/// Check one advanced setting's value.
///
/// # Errors
/// The name is not an advanced setting, or the value is outside what the
/// driver accepts for it.
pub(crate) fn check_advanced(name: &str, value: &str) -> Result<(), String> {
    advanced(name)?.check(value)
}

/// The SSL mode to offer for a host: `require` for a remote host, `prefer` for
/// this machine.
///
/// A remote host defaults to `require` because that is what providers expect
/// and because certificate checking is then not optional. This machine defaults
/// to `prefer` so an unencrypted local server keeps working.
#[must_use]
pub(crate) fn default_ssl_for(host: &str) -> SslChoice {
    let host = host.split(',').map(str::trim).find(|part| !part.is_empty());
    match host {
        Some(host) if is_loopback(host) => SslChoice::Prefer,
        _ => SslChoice::Require,
    }
}

/// The `sslmode` an address asks for, if it asks for one.
fn ssl_mode_of(url: &str) -> Option<&str> {
    let (_, after) = url.split_once("://")?;
    let query = after.split_once('?')?.1;
    query.split('&').find_map(|pair| {
        let (name, value) = pair.split_once('=')?;
        (name.eq_ignore_ascii_case("sslmode")).then_some(value)
    })
}

#[cfg(test)]
#[path = "setup_validate_tests.rs"]
mod tests;
