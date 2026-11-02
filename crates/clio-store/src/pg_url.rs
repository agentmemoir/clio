// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the SSL options a Postgres connection string
//! carries: the mode the operator asked for, an optional root certificate, and
//! the string the driver's own parser can accept.
//!
//! ## Owns
//! - Reading `sslmode` and `sslrootcert` out of either connection form
//! - The mode a string that names none defaults to, which depends on its hosts
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
//! Changes to how an SSL option is spelled in a connection string should
//! normally require changes here. Changes to the connector, the roots, or the
//! address fields should not.
//!
//! # Why the driver is not asked to read these
//!
//! The driver's parser knows three `sslmode` values and no `sslrootcert` at
//! all, and it refuses anything else as an unknown option. So the two keys this
//! store owns are taken out at their own token boundaries, the rest of the
//! string is handed to the driver untouched, and the mode is set on the parsed
//! configuration. Nothing else is rewritten: a `?`, `&`, or `://` inside a
//! password stays exactly where the operator wrote it.

use crate::pg_tls::SslChoice;
use postgres::Config;
use postgres::config::SslMode;
use std::str::FromStr;

/// The parameter that carries the SSL mode.
const SSL_MODE_PARAM: &str = "sslmode";

/// The parameter that carries a caller-supplied root certificate.
const SSL_ROOT_PARAM: &str = "sslrootcert";

/// The SSL options a connection string carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TlsSpec {
    /// The string with only the store's two keys removed, ready for the
    /// driver's own parser. The mode is set on the configuration, so this
    /// string carries no mode.
    pub(crate) driver_url: String,
    /// The mode the operator asked for, or the host-aware default.
    pub(crate) ssl: SslChoice,
    /// The root certificate the string named, when it named a non-empty one.
    pub(crate) root: Option<String>,
}

/// Read the SSL options from a connection string.
///
/// A string that names no mode gets [`default_mode`]: `verify-full` for a host
/// off this machine, `prefer` when every host is local. An explicit `sslmode`
/// is read through [`SslChoice::parse`] and wins over that default.
///
/// # Errors
/// The `sslmode` names a mode this store does not offer.
pub(crate) fn spec(input: &str) -> Result<TlsSpec, String> {
    let options = Options::read(input);
    let ssl = match options.mode {
        Some(raw) => SslChoice::parse(&raw)?,
        None => default_mode(&options.kept),
    };
    Ok(TlsSpec {
        driver_url: options.kept,
        ssl,
        root: options.root.filter(|path| !path.is_empty()),
    })
}

/// The value the driver's own parser understands for one of this store's modes.
///
/// The driver knows `disable`, `prefer`, and `require` only. Both
/// `verify-full` and `encrypt-no-verify` mean "require TLS" to it; the chosen
/// connector is what makes one verify the certificate and the other not.
pub(crate) fn driver_mode(ssl: SslChoice) -> SslMode {
    match ssl {
        SslChoice::Disable => SslMode::Disable,
        SslChoice::Prefer => SslMode::Prefer,
        SslChoice::VerifyFull | SslChoice::EncryptNoVerify => SslMode::Require,
    }
}

/// The mode a string that names none defaults to.
///
/// A host off this machine gets `verify-full`: the operator did not ask for
/// less, and a network the operator does not control is where checking the
/// server matters. A Unix socket is on this machine by definition, and so is a
/// loopback name, so those keep `prefer` and an unencrypted local server keeps
/// working. An empty host list also keeps `prefer`: the driver refuses such a
/// string before TLS is reached, and a string the driver cannot read at all
/// keeps it for the same reason.
fn default_mode(input: &str) -> SslChoice {
    let Ok(config) = Config::from_str(input) else {
        return SslChoice::Prefer;
    };
    if crate::pg_tls::config_is_local(&config) {
        SslChoice::Prefer
    } else {
        SslChoice::VerifyFull
    }
}

/// The store-owned keys taken out of one connection string.
struct Options {
    /// The string with those keys removed, and nothing else changed.
    kept: String,
    /// The last `sslmode` value, when the string named one.
    mode: Option<String>,
    /// The last `sslrootcert` value, decoded, when the string named one.
    root: Option<String>,
}

impl Options {
    /// Read either connection form the driver accepts.
    ///
    /// The form is chosen the way the driver chooses it: only the two Postgres
    /// schemes select the URL form, and everything else is key/value.
    fn read(input: &str) -> Self {
        if input.starts_with("postgres://") || input.starts_with("postgresql://") {
            Self::from_url(input)
        } else {
            Self::from_key_value(input)
        }
    }

    /// Read the query of a URL, leaving its path and credentials untouched.
    fn from_url(input: &str) -> Self {
        let Some(query_start) = url_query_start(input) else {
            return Self {
                kept: input.to_owned(),
                mode: None,
                root: None,
            };
        };
        let mut kept: Vec<&str> = Vec::new();
        let mut mode = None;
        let mut root = None;
        for pair in input[query_start + 1..].split('&') {
            let Some((key, value)) = pair.split_once('=') else {
                // Leave a malformed pair for the driver to refuse.
                kept.push(pair);
                continue;
            };
            if key.eq_ignore_ascii_case(SSL_MODE_PARAM) {
                mode = Some(percent_decode(value));
            } else if key.eq_ignore_ascii_case(SSL_ROOT_PARAM) {
                root = Some(percent_decode(value));
            } else {
                kept.push(pair);
            }
        }
        let mut driver_url = input[..query_start].to_owned();
        if !kept.is_empty() {
            driver_url.push('?');
            driver_url.push_str(&kept.join("&"));
        }
        Self {
            kept: driver_url,
            mode,
            root,
        }
    }

    /// Read the whitespace-separated pairs of a key/value string.
    ///
    /// The scan follows the driver's grammar closely enough to tell a key from
    /// a quoted password: keys start after whitespace, an `=` may have spaces
    /// around it, and a quoted value is skipped whole.
    fn from_key_value(input: &str) -> Self {
        let bytes = input.as_bytes();
        let mut kept = String::new();
        let mut mode = None;
        let mut root = None;
        let mut i = 0;
        while i < bytes.len() {
            while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            if i >= bytes.len() {
                break;
            }
            let start = i;
            while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b'=' {
                i += 1;
            }
            let key = &input[start..i];
            let mut equals = i;
            while equals < bytes.len() && bytes[equals].is_ascii_whitespace() {
                equals += 1;
            }
            if equals >= bytes.len() || bytes[equals] != b'=' {
                // Keep the unreadable rest so the driver still refuses it.
                push_param(&mut kept, &input[start..]);
                break;
            }
            let mut value_start = equals + 1;
            while value_start < bytes.len() && bytes[value_start].is_ascii_whitespace() {
                value_start += 1;
            }
            let (value, end) = read_value(input, value_start);
            if key.eq_ignore_ascii_case(SSL_MODE_PARAM) {
                mode = Some(value);
            } else if key.eq_ignore_ascii_case(SSL_ROOT_PARAM) {
                root = Some(value);
            } else {
                push_param(&mut kept, &input[start..end]);
            }
            i = end;
        }
        Self { kept, mode, root }
    }
}

/// Read one value in the key/value form, returning it and where it ends.
fn read_value(input: &str, start: usize) -> (String, usize) {
    let bytes = input.as_bytes();
    if bytes.get(start) != Some(&b'\'') {
        let mut end = start;
        while end < bytes.len() && !bytes[end].is_ascii_whitespace() {
            end += 1;
        }
        return (input[start..end].to_owned(), end);
    }
    let inner = start + 1;
    let mut end = inner;
    while end < bytes.len() {
        if bytes[end] == b'\\' {
            end = (end + 2).min(bytes.len());
        } else if bytes[end] == b'\'' {
            break;
        } else {
            end += 1;
        }
    }
    let value = input[inner..end].to_owned();
    let after = if end < bytes.len() { end + 1 } else { end };
    (value, after)
}

/// Append one untouched parameter to a rebuilt key/value string.
fn push_param(kept: &mut String, param: &str) {
    if !kept.is_empty() {
        kept.push(' ');
    }
    kept.push_str(param);
}

/// The byte index of the `?` that starts a URL's query, when it has one.
///
/// The scan follows the driver's own order: credentials run to the first `@`,
/// the host to the next `/` or `?`, and the query to the next `?` after a path.
/// That is why a `?` or `://` inside a password cannot be mistaken for the
/// start of the options.
fn url_query_start(input: &str) -> Option<usize> {
    let prefix = if input.starts_with("postgresql://") {
        "postgresql://"
    } else {
        "postgres://"
    };
    let rest_start = prefix.len();
    let rest = &input[rest_start..];
    let (after_start, after) = match rest.find('@') {
        Some(at) => (rest_start + at + 1, &rest[at + 1..]),
        None => (rest_start, rest),
    };
    let at = after.find(['/', '?'])?;
    if after.as_bytes()[at] == b'?' {
        return Some(after_start + at);
    }
    after[at..].find('?').map(|query| after_start + at + query)
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
