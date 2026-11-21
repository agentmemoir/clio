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

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// The host of the built-in external memory provider endpoint.
///
/// It is the one destination the shipped configuration allows without any
/// operator setup, so the provider keeps working out of the box. Defined here
/// because both the config default and the destination policy have to agree on
/// it, and this crate is the only one they both depend on.
pub const BUILT_IN_PROVIDER_HOST: &str = "api.mem0.ai";

/// The address an IPv6 address inside `64:ff9b::/96` carries in its last four
/// bytes, when the address is inside that prefix.
///
/// IANA marks the prefix Globally Reachable, no std predicate flags it, and
/// `to_canonical` leaves it unchanged, so this decode is the only way such an
/// address faces the IPv4 tests at all.
fn nat64_embedded(ip: Ipv6Addr) -> Option<Ipv4Addr> {
    let segments = ip.segments();
    let inside_prefix = segments[0] == 0x0064
        && segments[1] == 0xff9b
        && segments[2..6].iter().all(|segment| *segment == 0);
    if !inside_prefix {
        return None;
    }
    let octets = ip.octets();
    Some(Ipv4Addr::new(
        octets[12], octets[13], octets[14], octets[15],
    ))
}

/// True when `ip` is an address a server must not be aimed at on behalf of a
/// caller: this machine, a private network, a link-local segment, a cloud
/// metadata endpoint, or a range reserved for something that is not a public
/// peer.
///
/// The IPv6 form is canonicalised first, so an IPv4-mapped address
/// (`::ffff:127.0.0.1`) faces the IPv4 tests as the IPv4 address it is. IPv4
/// ranges that std has no stable predicate for are read from their masks.
#[must_use]
pub fn is_denied_destination(ip: IpAddr) -> bool {
    match ip.to_canonical() {
        IpAddr::V4(ipv4) => is_denied_ipv4(ipv4),
        IpAddr::V6(ipv6) => {
            is_denied_ipv6(ipv6) || nat64_embedded(ipv6).is_some_and(is_denied_ipv4)
        }
    }
}

fn is_denied_ipv4(ip: Ipv4Addr) -> bool {
    if ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_multicast()
        || ip.is_broadcast()
        || ip.is_documentation()
    {
        return true;
    }
    let [first, second, _, _] = ip.octets();
    // Carrier-grade NAT, the benchmarking range, the reserved class E block,
    // and "this network" all name something that is not a public peer.
    (first == 100 && (second & 0xc0) == 64)
        || (first == 198 && (second & 0xfe) == 18)
        || (first & 0xf0) == 240
        || first == 0
}

fn is_denied_ipv6(ip: Ipv6Addr) -> bool {
    ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || ip.is_unique_local()
        || ip.is_unicast_link_local()
}

/// `url` with a `user:password@` userinfo replaced by `***`, so a refusal can
/// echo the address without echoing the password in it.
///
/// A userinfo carrying only a user name is left alone: that address holds no
/// password, and marking it would put a redaction mark on a value that never
/// had a secret. The colon is what separates the password from the user name.
fn mask_userinfo(url: &str) -> String {
    let Some((head, tail)) = url.split_once("://") else {
        return url.to_owned();
    };
    let Some((userinfo, rest)) = tail.split_once('@') else {
        return url.to_owned();
    };
    if userinfo.contains(':') {
        format!("{head}://***@{rest}")
    } else {
        url.to_owned()
    }
}

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
/// literal are syntax. `http://trusted@evil.example.com` yields `evil.example.com`,
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
    let shown = mask_userinfo(trimmed);
    let scheme = url_scheme(trimmed)
        .ok_or_else(|| format!("url `{shown}` has no usable `scheme://` prefix"))?;
    let (_, after_prefix) = trimmed
        .split_once("://")
        .ok_or_else(|| format!("url `{shown}` has no usable `scheme://` prefix"))?;
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
                return Err(format!("url `{shown}` has an unterminated bracketed host"));
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
        return Err(format!("url `{shown}` has an empty host"));
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
            "url `{}` must be http:// or https://, got `{scheme}://`",
            mask_userinfo(url)
        ));
    }
    Ok(HttpTarget { scheme, host })
}

#[cfg(test)]
#[path = "net_tests.rs"]
mod net_tests;
