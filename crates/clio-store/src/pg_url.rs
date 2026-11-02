// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the SSL options a Postgres connection URL
//! carries: the mode the operator asked for, an optional root certificate, and
//! the URL the driver's own parser can accept.
//!
//! ## Owns
//! - Reading `sslmode` and `sslrootcert` out of a URL query
//! - The mode a URL that names none defaults to, which depends on its hosts
//! - Mapping a mode onto the value the driver's own parser knows
//! - Percent-decoding a supplied root certificate path
//!
//! ## Does not own
//! - What a mode means at connect time (see `pg_tls`, which picks the connector)
//! - Loading or trusting a certificate (`tls_trust` owns that)
//! - The host names themselves (the driver's parser is asked for them)
//!
//! ## Boundary
//!
//! Changes to how an SSL option is spelled in a URL should normally require
//! changes here. Changes to the connector, the roots, or the address fields
//! should not.
//!
//! # Why the driver cannot read these itself
//!
//! The driver's parser knows three `sslmode` values and no `sslrootcert` at
//! all, and it refuses anything else as an unknown option. So the URL the
//! operator wrote is read here, `sslrootcert` is taken out, and `sslmode` is
//! rewritten to a value the driver accepts before it ever sees the string. The
//! original spelling is kept for the mode this store acts on.

use crate::pg_tls::SslChoice;
use clio_types::is_loopback_host;
use std::str::FromStr;
use tokio_postgres::Config;
use tokio_postgres::config::Host;

/// The query parameter that carries the SSL mode.
const SSL_MODE_PARAM: &str = "sslmode";

/// The query parameter that carries a caller-supplied root certificate.
const SSL_ROOT_PARAM: &str = "sslrootcert";

/// The SSL options a connection URL carries, and the URL the driver can parse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TlsSpec {
    /// The URL with `sslmode` mapped to a driver value and `sslrootcert`
    /// removed, so the driver's own parser accepts it.
    pub(crate) driver_url: String,
    /// The mode the operator asked for.
    pub(crate) ssl: SslChoice,
    /// The root certificate the URL named, when it named a non-empty one.
    pub(crate) root: Option<String>,
}

/// Read the SSL options from a connection URL.
///
/// A key/value connection string carries no `?` query and is left for the
/// driver alone; only the URL form carries a root certificate. The mode is read
/// from either form.
///
/// A string that names no mode gets [`default_mode`]: `verify-full` for a host
/// off this machine, `prefer` when every host is local. When that default is
/// `verify-full` the driver URL is rewritten to force the negotiation, because
/// otherwise the driver would open with its own weaker default.
///
/// # Errors
/// The `sslmode` names a mode this store does not offer.
pub(crate) fn spec(url: &str) -> Result<TlsSpec, String> {
    let Some((base, query)) = url.split_once('?') else {
        let requested = mode_in_query_or_text(url);
        let ssl = match requested {
            Some(raw) => SslChoice::parse(raw)?,
            None => default_mode(url),
        };
        let mut driver_url = url.to_owned();
        // A key/value string is separated by spaces, so the mode joins it the
        // same way. The driver reads both forms.
        if requested.is_none() && ssl == SslChoice::VerifyFull {
            driver_url.push(if url.contains("://") { '?' } else { ' ' });
            driver_url.push_str(SSL_MODE_PARAM);
            driver_url.push('=');
            driver_url.push_str(driver_mode(ssl));
        }
        return Ok(TlsSpec {
            driver_url,
            ssl,
            root: None,
        });
    };
    let mut kept: Vec<&str> = Vec::new();
    let mut requested: Option<&str> = None;
    let mut root: Option<String> = None;
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        if key.eq_ignore_ascii_case(SSL_MODE_PARAM) {
            requested = Some(value);
        } else if key.eq_ignore_ascii_case(SSL_ROOT_PARAM) {
            let path = percent_decode(value);
            if !path.is_empty() {
                root = Some(path);
            }
        } else {
            kept.push(pair);
        }
    }
    let ssl = match requested {
        Some(raw) => SslChoice::parse(raw)?,
        None => default_mode(base),
    };
    // The driver needs one of its own three values, so the rewritten URL says
    // `require` for both verified and unverified TLS and the connector alone
    // decides whether the certificate is checked.
    let mut driver_url = String::from(base);
    driver_url.push('?');
    for pair in &kept {
        driver_url.push_str(pair);
        driver_url.push('&');
    }
    driver_url.push_str(SSL_MODE_PARAM);
    driver_url.push('=');
    driver_url.push_str(driver_mode(ssl));
    Ok(TlsSpec {
        driver_url,
        ssl,
        root,
    })
}

/// The value the driver's own parser understands for one of this store's modes.
///
/// The driver knows `disable`, `prefer`, and `require` only. Both
/// `verify-full` and `encrypt-no-verify` mean "require TLS" to it; the chosen
/// connector is what makes one verify the certificate and the other not.
fn driver_mode(ssl: SslChoice) -> &'static str {
    match ssl {
        SslChoice::Disable => "disable",
        SslChoice::Prefer => "prefer",
        SslChoice::VerifyFull | SslChoice::EncryptNoVerify => "require",
    }
}

/// The mode a string that names none defaults to.
///
/// A host off this machine gets `verify-full`: the operator did not ask for
/// less, and a network the operator does not control is where checking the
/// server matters. A Unix socket is on this machine by definition, and so is a
/// loopback name, so those keep `prefer` and an unencrypted local server keeps
/// working. An empty host list also keeps `prefer`, because the driver refuses
/// such a string before TLS is reached; a string the driver cannot read at all
/// keeps it for the same reason.
fn default_mode(url: &str) -> SslChoice {
    let Ok(config) = Config::from_str(url) else {
        return SslChoice::Prefer;
    };
    let local = config.get_hosts().iter().all(|host| match host {
        Host::Tcp(name) => is_loopback_host(name),
        Host::Unix(_) => true,
    });
    if local {
        SslChoice::Prefer
    } else {
        SslChoice::VerifyFull
    }
}

/// The `sslmode` a string asks for, from a URL query or a key/value text.
fn mode_in_query_or_text(url: &str) -> Option<&str> {
    if let Some((_, after)) = url.split_once("://") {
        let query = after.split_once('?')?.1;
        return find_param(query, SSL_MODE_PARAM);
    }
    url.split(' ').find_map(|token| {
        let (name, value) = token.split_once('=')?;
        name.eq_ignore_ascii_case(SSL_MODE_PARAM).then_some(value)
    })
}

/// The value of one query parameter, matched without regard to case.
fn find_param<'a>(query: &'a str, name: &str) -> Option<&'a str> {
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        key.eq_ignore_ascii_case(name).then_some(value)
    })
}

/// Decode `%XX` escapes, leaving anything else as written.
///
/// The wizard percent-encodes a path before it goes into the URL, so the path
/// read back has to be decoded before it can name a file. Bytes that do not
/// form a valid escape are kept byte for byte, which is what the driver's own
/// decoder does.
fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(high), Some(low)) = (hex(bytes[i + 1]), hex(bytes[i + 2]))
        {
            out.push((high << 4) | low);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// One hexadecimal digit, or nothing.
fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
#[path = "pg_url_tests.rs"]
mod pg_url_tests;
