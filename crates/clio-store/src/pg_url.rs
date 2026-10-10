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
//! - Mapping a mode onto the value the driver's own parser knows
//! - Percent-decoding a supplied root certificate path
//!
//! ## Does not own
//! - What a mode means at connect time (see `pg_tls`, which picks the connector)
//! - Loading or trusting a certificate (`tls_trust` owns that)
//! - Hosts, ports, or credentials (the driver's parser owns those)
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
/// # Errors
/// The `sslmode` names a mode this store does not offer.
pub(crate) fn spec(url: &str) -> Result<TlsSpec, String> {
    let Some((base, query)) = url.split_once('?') else {
        let ssl = match mode_in_query_or_text(url) {
            Some(raw) => SslChoice::parse(raw)?,
            None => SslChoice::Prefer,
        };
        return Ok(TlsSpec {
            driver_url: url.to_owned(),
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
        None => SslChoice::Prefer,
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
