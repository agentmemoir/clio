// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the shared denied-destination policy,
//! the loopback rule, and the scheme/host splits every gate in the workspace
//! asks.
//!
//! ## Owns
//! - The loopback boundary: which names and addresses count as this machine
//! - One case per denied address class, including the NAT64 prefixes and the
//!   IPv4-mapped form that only a canonical or decoded read exposes
//! - The userinfo, port, and IPv6 cases a hand-rolled split gets wrong
//! - The strict split that refuses a spelling two URL parsers could read as
//!   different hosts
//!
//! ## Does not own
//! - The allowlist and the refusal wording (`clio-mcp` owns the provider gate,
//!   `clio-config` and `clio-index` own their own)
//! - Sending anything over a socket
//!
//! ## Boundary
//!
//! Changes to the loopback rule, the denied classes, or either split should
//! normally require changes here. Changes to a caller's wording should not
//! require changes here.

use crate::net::{
    http_target, is_denied_destination, is_loopback_host, parse_scheme_host, strict_http_target,
};
use std::net::IpAddr;

#[test]
fn loopback_names_and_addresses_are_this_machine() {
    for host in [
        "localhost",
        "LOCALHOST",
        "LocalHost",
        "box.localhost",
        "a.b.c.localhost",
        "127.0.0.1",
        // 127.x is loopback: the whole 127.0.0.0/8 range is this machine, not
        // just 127.0.0.1.
        "127.1.2.3",
        "127.255.255.254",
        "::1",
        "[::1]",
        "0:0:0:0:0:0:0:1",
        "::1%lo0",
        "[::1%lo0]",
    ] {
        assert!(is_loopback_host(host), "{host} must be loopback");
    }
}

#[test]
fn remote_names_and_addresses_are_not_this_machine() {
    for host in [
        "db.example.com",
        "localhost.example.com",
        // A name that merely ends in `.localhost` is fine; one that merely
        // starts with `localhost` is a different host.
        "localhost.evil.example.com",
        "notlocalhost",
        "10.0.0.1",
        "192.168.1.10",
        "126.255.255.255",
        "128.0.0.1",
        "fe80::1",
        "[fe80::1%en0]",
        "2001:db8::1",
        // An IPv4-mapped loopback is still not an IPv6 loopback literal.
        // An IPv4-mapped address is a different address from `::1`, and a
        // connection to it is not an IPv6 loopback connection.
        "::ffff:127.0.0.1",
        "",
        "   ",
    ] {
        assert!(!is_loopback_host(host), "{host} must not be loopback");
    }
}

#[test]
fn userinfo_is_never_a_loopback_name() {
    // The last `@` ends the userinfo, so what follows is the real host. None of
    // these may be accepted just because a loopback-looking name sits in front.
    for host in [
        "trusted@evil.example.com",
        "localhost@evil.example.com",
        "box.localhost@evil.example.com",
        "127.0.0.1@evil.example.com",
        "evil.example.com@localhost",
        "localhost@",
    ] {
        assert!(!is_loopback_host(host), "{host} must not be loopback");
    }
}

#[test]
fn split_reads_scheme_and_the_host_that_would_be_reached() {
    let (scheme, host) = parse_scheme_host("HTTPS://Embed.Example.COM:8443/v1/embed").unwrap();
    assert_eq!(scheme, "https");
    assert_eq!(host, "embed.example.com");

    let (scheme, host) = parse_scheme_host("http://127.0.0.1:34311/embed").unwrap();
    assert_eq!(scheme, "http");
    assert_eq!(host, "127.0.0.1");

    // The smuggled case: the userinfo is not the host.
    let (scheme, host) = parse_scheme_host("http://trusted@evil.example.com/x").unwrap();
    assert_eq!(scheme, "http");
    assert_eq!(host, "evil.example.com");

    // A `?` or `#` ends the authority even without a `/`.
    let (_, host) = parse_scheme_host("http://box.localhost?a=b").unwrap();
    assert_eq!(host, "box.localhost");
    let (_, host) = parse_scheme_host("http://box.localhost#frag").unwrap();
    assert_eq!(host, "box.localhost");

    // Bracketed IPv6 keeps every byte of the address.
    let (_, host) = parse_scheme_host("http://[2001:db8::1]:34311/embed").unwrap();
    assert_eq!(host, "2001:db8::1");
    let (_, host) = parse_scheme_host("http://[::1]/embed").unwrap();
    assert_eq!(host, "::1");
}

#[test]
fn split_refuses_what_it_cannot_read() {
    for bad in [
        "no-scheme.example.com",
        "://missing-scheme",
        "ht tp://spaced",
        "http://",
        "http://[::1/embed",
    ] {
        assert!(
            parse_scheme_host(bad).is_err(),
            "{bad} must be refused, got {:?}",
            parse_scheme_host(bad)
        );
    }
}

#[test]
fn a_split_refusal_masks_the_password_in_the_url() {
    let error = parse_scheme_host("http://u:topsecret@[::1/x").expect_err("no bracket");
    assert!(
        !error.contains("topsecret"),
        "the password is not echoed: {error}"
    );
    assert!(
        error.contains("***@[::1/x"),
        "the address is still named: {error}"
    );
}

#[test]
fn an_http_target_demands_an_http_scheme() {
    let target = http_target("http://127.0.0.1:34311").expect("loopback is readable");
    assert_eq!(target.scheme, "http");
    assert_eq!(target.host, "127.0.0.1");
    assert!(target.is_loopback());

    let target = http_target("https://api.example.com/v1").expect("remote https is readable");
    assert_eq!(target.host, "api.example.com");
    // `https` to a remote host is encrypted, so this pair is not the plain-http
    // case any gate refuses.
    assert!(!target.is_loopback());

    for bad in [
        "ftp://api.example.com",
        "postgres://db.example.com/clio",
        "api.example.com",
    ] {
        assert!(http_target(bad).is_err(), "{bad} must be refused");
    }
}

#[test]
fn a_scheme_refusal_masks_the_password_in_the_url() {
    let error =
        http_target("postgres://u:topsecret@db.example.com/clio").expect_err("not an http target");
    assert!(
        !error.contains("topsecret"),
        "the password is not echoed: {error}"
    );
}

#[test]
fn a_passwordless_url_is_echoed_unchanged() {
    let error = http_target("postgres://db.example.com/clio").expect_err("not an http target");
    assert!(error.contains("postgres://db.example.com/clio"), "{error}");
}

/// One entry per denial class the destination rule has to cover, so a class that
/// stops being denied is a failing test rather than a silently reachable host.
#[test]
fn every_non_public_address_class_is_a_denied_destination() {
    for text in [
        // Loopback, private, link-local (cloud metadata), unspecified, and the
        // reserved classes std has a stable predicate for.
        "127.0.0.1",
        "127.255.255.254",
        "10.0.0.1",
        "172.16.0.1",
        "192.168.1.10",
        "169.254.169.254",
        "0.0.0.0",
        "224.0.0.1",
        "255.255.255.255",
        "192.0.2.1",
        // Carrier-grade NAT, benchmarking, class E, and "this network", which
        // have no stable predicate and are read from their masks.
        "100.64.0.1",
        "100.127.255.254",
        "198.18.0.1",
        "198.19.255.254",
        "240.0.0.1",
        "0.1.2.3",
        // IPv6 loopback, unspecified, multicast, unique-local, and link-local.
        "::1",
        "::",
        "ff02::1",
        "fd00::1",
        "fc00::1",
        "fe80::1",
    ] {
        let Ok(ip) = text.parse::<IpAddr>() else {
            panic!("case is not an address: {text}");
        };
        assert!(is_denied_destination(ip), "{text} must be denied");
    }
}

/// An IPv4-mapped address is not an IPv6 loopback literal, and `is_loopback`
/// alone says no to it; only canonicalising first exposes the embedded IPv4
/// address that makes it a denial.
#[test]
fn an_ipv4_mapped_address_is_denied_through_its_canonical_form() {
    for text in [
        "::ffff:127.0.0.1",
        "::ffff:169.254.169.254",
        "::ffff:10.0.0.1",
        "::ffff:100.64.0.1",
    ] {
        let ip = text.parse::<IpAddr>().expect("address");
        assert!(is_denied_destination(ip), "{text} must be denied");
    }
    // The public counterpart is what proves the mapping is tested as IPv4 rather
    // than denied for being an IPv6 literal at all.
    let public = "::ffff:93.184.216.34".parse::<IpAddr>().expect("address");
    assert!(!is_denied_destination(public));
}

/// The NAT64 prefixes are Globally Reachable on paper or carry no predicate at
/// all, so no `is_loopback`/`is_private`-style test and no canonicalisation flags
/// them; only decoding the embedded IPv4 address does. Both the `/96` prefix and
/// the `/48` prefix reserved for local use embed the address in the last 32 bits.
#[test]
fn a_nat64_address_is_denied_through_its_embedded_ipv4_address() {
    for text in [
        // 169.254.169.254, 127.0.0.1, 10.0.0.1, 100.64.0.1.
        "64:ff9b::a9fe:a9fe",
        "64:ff9b::7f00:1",
        "64:ff9b::a00:1",
        "64:ff9b::6440:1",
        // The same four addresses through `64:ff9b:1::/48`, which is reserved for
        // local use and marked not globally reachable. Its segment before the
        // embedded address is not zero, so only a `/48` decode catches it.
        "64:ff9b:1::a9fe:a9fe",
        "64:ff9b:1::7f00:1",
        "64:ff9b:1::a00:1",
        "64:ff9b:1::6440:1",
        "64:ff9b:1:ffff::a9fe:a9fe",
    ] {
        let ip = text.parse::<IpAddr>().expect("address");
        assert!(is_denied_destination(ip), "{text} must be denied");
    }
    // 93.184.216.34 again, this time through both NAT64 prefixes: the decode has
    // to let a public embedded address through, or every NAT64 peer would be
    // refused.
    for text in ["64:ff9b::5db8:d822", "64:ff9b:1::5db8:d822"] {
        let public = text.parse::<IpAddr>().expect("address");
        assert!(!is_denied_destination(public), "{text} must not be denied");
    }
    // A prefix that embeds no IPv4 address is not decoded at all, so the bytes in
    // its last two segments are not read as one.
    let other_prefix = "64:ff9b:2::7f00:1".parse::<IpAddr>().expect("address");
    assert!(!is_denied_destination(other_prefix));
}

/// A gate and the transport are two parsers over one string, and OWASP's rule is
/// that a disagreement between them is a rejection rather than something to
/// reconcile. The demonstrated input is the default allowlisted host inside a
/// userinfo that the transport keeps as part of its host.
#[test]
fn a_strict_target_refuses_an_authority_two_parsers_could_read_differently() {
    for bad in [
        "http://127.0.0.1:34311\\@api.mem0.ai/v1",
        "http://127.0.0.1:34311\\@api.mem0.ai",
        "http://127.0.0.1\\@api.mem0.ai/v1",
        "http://api.mem0.ai\\@127.0.0.1/v1",
        "http://user:pw@api.mem0.ai/v1",
        "http://api.mem0.ai@127.0.0.1/v1",
        "http://api%2emem0%2eai/v1",
        "http://api.mem0.ai\u{0}/v1",
        "http://api.mem0.ai\t/v1",
    ] {
        assert!(
            strict_http_target(bad).is_err(),
            "`{bad}` must be refused, got {:?}",
            strict_http_target(bad)
        );
    }
}

/// The strict split must not refuse the ordinary spellings, or the gate would
/// have no reachable provider at all.
#[test]
fn a_strict_target_reads_the_ordinary_spellings() {
    for (url, expected) in [
        ("http://127.0.0.1:34311/v1", "127.0.0.1"),
        ("https://api.mem0.ai/v1/memories", "api.mem0.ai"),
        ("HTTPS://API.Mem0.AI:8443/v1", "api.mem0.ai"),
        ("https://api.mem0.ai./v1", "api.mem0.ai."),
        ("http://[::1]:34311/v1", "::1"),
        ("https://api.mem0.ai?x=1", "api.mem0.ai"),
        ("https://api.mem0.ai#frag", "api.mem0.ai"),
    ] {
        let target = strict_http_target(url).unwrap_or_else(|e| panic!("`{url}`: {e}"));
        assert_eq!(target.host, expected, "{url}");
    }
}

#[test]
fn a_public_address_is_not_a_denied_destination() {
    // If this ever fails the tool is unusable over the internet, which is the
    // regression the deny list has to be bounded by.
    for text in [
        "93.184.216.34",
        "1.1.1.1",
        "8.8.8.8",
        "2001:4860:4860::8888",
    ] {
        let ip = text.parse::<IpAddr>().expect("address");
        assert!(!is_denied_destination(ip), "{text} must not be denied");
    }
}
