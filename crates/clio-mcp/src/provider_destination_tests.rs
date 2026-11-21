// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the destination-trust rule a provider
//! base URL has to pass before the adapter builds a request for it.
//!
//! ## Owns
//! - One rejection per address class, plus the allowlist and scheme decisions
//! - The trailing-dot and case normalisation the allowlist compare depends on
//!
//! ## Does not own
//! - Address classification itself (`clio-types/src/net_tests.rs`)
//! - The adapter's use of the rule (`provider_adapter_destination_tests.rs`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use clio_types::ErrorCode;

use super::{check_provider_target, normalise_host};

/// The refusals every non-allowlisted destination receives, so a test states the
/// rule it expects rather than one wording.
fn refused(url: &str) -> clio_types::AmError {
    check_provider_target(url, &[]).expect_err("must be refused")
}

#[test]
fn an_ipv4_loopback_destination_is_refused() {
    let err = refused("http://127.0.0.1:8080/v1");
    assert_eq!(err.code, ErrorCode::Forbidden);
    assert!(err.message.contains("127.0.0.1"), "{err}");
}

#[test]
fn an_ipv4_private_destination_is_refused() {
    let err = refused("https://10.0.0.1/v1");
    assert_eq!(err.code, ErrorCode::Forbidden);
    assert!(err.message.contains("10.0.0.1"), "{err}");
}

#[test]
fn a_metadata_link_local_destination_is_refused() {
    let err = refused("http://169.254.169.254/latest/meta-data/");
    assert_eq!(err.code, ErrorCode::Forbidden);
    assert!(err.message.contains("169.254.169.254"), "{err}");
}

#[test]
fn an_ipv4_carrier_grade_nat_destination_is_refused() {
    let err = refused("http://100.64.0.1/v1");
    assert_eq!(err.code, ErrorCode::Forbidden);
    assert!(err.message.contains("100.64.0.1"), "{err}");
}

#[test]
fn an_ipv6_loopback_destination_is_refused() {
    let err = refused("http://[::1]:8080/v1");
    assert_eq!(err.code, ErrorCode::Forbidden);
    assert!(err.message.contains("::1"), "{err}");
}

#[test]
fn an_ipv6_unique_local_destination_is_refused() {
    let err = refused("http://[fd00::1]/v1");
    assert_eq!(err.code, ErrorCode::Forbidden);
    assert!(err.message.contains("fd00::1"), "{err}");
}

#[test]
fn an_ipv6_link_local_destination_is_refused() {
    let err = refused("http://[fe80::1]/v1");
    assert_eq!(err.code, ErrorCode::Forbidden);
    assert!(err.message.contains("fe80::1"), "{err}");
}

#[test]
fn an_ipv4_mapped_loopback_destination_is_refused() {
    // This is not an IPv6 loopback literal, so only the canonical form exposes
    // the embedded IPv4 loopback that makes it a denial.
    let err = refused("http://[::ffff:127.0.0.1]/v1");
    assert_eq!(err.code, ErrorCode::Forbidden);
}

#[test]
fn an_ipv4_mapped_metadata_destination_is_refused() {
    let err = refused("http://[::ffff:169.254.169.254]/v1");
    assert_eq!(err.code, ErrorCode::Forbidden);
}

#[test]
fn a_nat64_metadata_destination_is_refused() {
    // 64:ff9b::a9fe:a9fe carries 169.254.169.254; the prefix is globally
    // reachable on paper, so only the decode catches it.
    let err = refused("http://[64:ff9b::a9fe:a9fe]/v1");
    assert_eq!(err.code, ErrorCode::Forbidden);
}

#[test]
fn an_allowlisted_host_is_accepted_even_when_its_address_is_private() {
    // An operator deliberately pointing at an internal provider must keep
    // working, so the allowlist outranks the address class.
    for url in [
        "http://127.0.0.1:8080/v1",
        "http://10.0.0.5/v1",
        "http://[fd00::1]/v1",
    ] {
        let target = check_provider_target(url, &["127.0.0.1", "10.0.0.5", "fd00::1"])
            .expect("an allowlisted host is accepted");
        let host = target.host;
        assert!(
            ["127.0.0.1", "10.0.0.5", "fd00::1"].contains(&host.as_str()),
            "{host}"
        );
    }
}

#[test]
fn the_allowlist_compare_ignores_case() {
    let target = check_provider_target("https://API.Example.COM/v1", &["api.example.com"])
        .expect("case is not part of the host");
    assert_eq!(target.host, "api.example.com");
}

#[test]
fn a_trailing_dot_does_not_escape_the_allowlist() {
    // `parse_scheme_host` keeps the trailing dot because it is part of the name
    // as typed, so a plain compare would refuse an allowlisted host spelled the
    // absolute way.
    let target = check_provider_target("https://api.mem0.ai./v1", &["api.mem0.ai"])
        .expect("one trailing dot names the same host");
    assert_eq!(target.host, "api.mem0.ai");
}

#[test]
fn a_bare_localhost_name_is_refused_by_address_class() {
    // `localhost` is a name, not an IP literal, so it reaches the allowlist
    // refusal; either way it never reaches a request without an operator entry.
    let err = refused("http://localhost.:8080/v1");
    assert_eq!(err.code, ErrorCode::Forbidden);
    assert!(err.message.contains("localhost"), "{err}");
}

#[test]
fn a_non_http_scheme_is_refused_before_any_address_rule() {
    for url in ["ftp://api.mem0.ai/v1", "postgres://db.example.com/clio"] {
        let err = refused(url);
        assert_eq!(err.code, ErrorCode::Forbidden, "{url}");
    }
}

#[test]
fn an_unlisted_public_host_is_refused() {
    let err = refused("https://attacker.example.com/v1");
    assert_eq!(err.code, ErrorCode::Forbidden);
    assert!(err.message.contains("attacker.example.com"), "{err}");
    assert!(err.message.contains("allowlist"), "{err}");
}

#[test]
fn normalisation_removes_one_trailing_dot_and_lowercases() {
    assert_eq!(normalise_host("API.Mem0.AI."), "api.mem0.ai");
    assert_eq!(normalise_host("api.mem0.ai"), "api.mem0.ai");
    // Only one dot goes, so a malformed name keeps the shape it was typed in.
    assert_eq!(normalise_host("api.mem0.ai.."), "api.mem0.ai.");
}
