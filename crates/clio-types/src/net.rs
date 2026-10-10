// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for answering one question about a network
//! address: is it this machine?
//!
//! ## Owns
//! - The loopback rule (`localhost`, `*.localhost`, `127.0.0.0/8`, `::1`)
//! - A scheme-and-host split that a config value can be checked with, when the
//!   crate that owns the transport is not a dependency of the caller
//!
//! ## Does not own
//! - Sending a request. `clio-index` owns the HTTP transport and its own parse
//! - Whether a connection string is well formed (`clio-store` asks the Postgres
//!   driver, which is the parser of record)
//! - The policy that refuses a remote plain address. Callers own their own
//!   wording; this module only answers the address question
//!
//! ## Boundary
//!
//! Changes to what counts as this machine should normally require changes here,
//! so the config gate, the request gate, and the store cannot drift apart into
//! three different answers. Changes to any one transport should not require
//! changes here.
//!
//! Keep this module focused on address classification with no I/O.

use std::net::IpAddr;

/// The lowercased scheme of `url`, or `None` when it carries no usable
/// `scheme://` prefix.
///
/// This is the one place a scheme is read, so a caller that needs only the
/// scheme (to refuse a `sqlite://` or `mysql://` target, say) does not have to
/// ask for a host it has no use for and that may not be readable.
#[must_use]
pub fn url_scheme(url: &str) -> Option<String> {
    let (scheme, _) = url.trim().split_once("://")?;
    let usable = !scheme.is_empty()
        && scheme
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'-' || b == b'.');
    usable.then(|| scheme.to_ascii_lowercase())
}

/// Split `url` into its lowercased scheme and lowercased host.
///
/// The host is what a connection would actually reach, so the split removes
/// the parts of the string that only look like one: a `user:password@` prefix
/// is not a host, a port is not part of it, and the brackets around an IPv6
/// literal are syntax. `http://trusted@evil.example` yields `evil.example`,
/// which is the point — a caller that asked for the host must not be handed the
/// userinfo instead.
///
/// The port is not read or checked. This answers "which host", and a malformed
/// port is a separate fault that the transport's own parse refuses.
///
/// # Errors
/// The string carries no usable `scheme://` prefix, an unterminated bracketed
/// IPv6 literal, or an empty host.
pub fn parse_scheme_host(url: &str) -> Result<(String, String), String> {
    let trimmed = url.trim();
    let scheme = url_scheme(trimmed)
        .ok_or_else(|| format!("url `{trimmed}` has no usable `scheme://` prefix"))?;
    let (_, after_prefix) = trimmed
        .split_once("://")
        .ok_or_else(|| format!("url `{trimmed}` has no usable `scheme://` prefix"))?;
    // The authority ends at the first delimiter a path, query, or fragment can
    // introduce; without this a `?` or `#` would become part of the host.
    let authority_end = after_prefix
        .find(['/', '?', '#'])
        .unwrap_or(after_prefix.len());
    let authority = &after_prefix[..authority_end];
    // Userinfo is everything up to the LAST `@`: an `@` is legal inside a
    // password, so the first one does not end it.
    let host_port = match authority.rsplit_once('@') {
        Some((_, after)) => after,
        None => authority,
    };
    let host = match host_port.strip_prefix('[') {
        // Bracketed IPv6 literal: "[host]" then optionally ":port".
        Some(after_open) => {
            let Some(close) = after_open.find(']') else {
                return Err(format!(
                    "url `{trimmed}` has an unterminated bracketed host"
                ));
            };
            &after_open[..close]
        }
        // Otherwise the last `:` starts the port, if there is one. A bare IPv6
        // literal has several colons, and this is exactly where a naive
        // `rsplit_once` would keep `::1` minus a byte; the bracketed form above
        // is the only way a host may contain a colon.
        None => host_port.rsplit_once(':').map_or(host_port, |(h, _)| h),
    };
    if host.is_empty() {
        return Err(format!("url `{trimmed}` has an empty host"));
    }
    Ok((scheme, host.to_ascii_lowercase()))
}

/// The scheme and host of an `http://` or `https://` URL, in that order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpTarget {
    /// `http` or `https`, lowercased.
    pub scheme: String,
    /// The host, lowercased, without userinfo, port, or IPv6 brackets.
    pub host: String,
}

impl HttpTarget {
    /// True when this address is on this machine.
    #[must_use]
    pub fn is_loopback(&self) -> bool {
        self.scheme == "http" && is_loopback_host(&self.host)
    }
}

/// True when `host` names this machine.
///
/// Loopback here is `localhost`, any `*.localhost` name, the whole
/// `127.0.0.0/8` range, and `::1`. A bracketed IPv6 literal is accepted with or
/// without its brackets, and an IPv6 zone id (`fe80::1%en0`) is ignored before
/// the address is read, because the zone selects a local interface and is not
/// part of the address being named.
///
/// A name carrying an `@` is never loopback, even when it ends in `.localhost`:
/// `trusted@box.localhost` reaches `box.localhost` only if something upstream
/// already decided to trust the `trusted` part, and this is the last place that
/// gets to decide. No real host name contains an `@` — the split above strips
/// userinfo — so refusing costs nothing.
#[must_use]
pub fn is_loopback_host(host: &str) -> bool {
    let host = host.trim();
    if host.contains('@') {
        return false;
    }
    let host = host
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or(host);
    // A zone id applies to link-local scopes and never changes which address is
    // named.
    let host = host.split('%').next().unwrap_or(host);
    if let Ok(ip) = host.parse::<IpAddr>() {
        return ip.is_loopback();
    }
    host.eq_ignore_ascii_case("localhost")
        || host
            .rsplit_once('.')
            .is_some_and(|(head, tail)| !head.is_empty() && tail.eq_ignore_ascii_case("localhost"))
}

/// Read an `http://` or `https://` URL as a scheme and host pair.
///
/// # Errors
/// The scheme is not `http` or `https`, or the string carries no readable host.
pub fn http_target(url: &str) -> Result<HttpTarget, String> {
    let (scheme, host) = parse_scheme_host(url)?;
    if scheme != "http" && scheme != "https" {
        return Err(format!(
            "url `{url}` must be http:// or https://, got `{scheme}://`"
        ));
    }
    Ok(HttpTarget { scheme, host })
}

#[cfg(test)]
#[path = "net_tests.rs"]
mod net_tests;
