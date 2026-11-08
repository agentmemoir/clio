// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the version text a release tag carries: the optional
//! leading `v`, the three numeric parts, the normalized rendering, and the
//! ordering two versions are compared by.
//!
//! ## Owns
//! - A leading `v` accepted, and never rendered
//! - Equal versions, and the ordering the three parts give
//! - Every text the strict reader must refuse
//!
//! ## Does not own
//! - Reading the release tag that carries a version
//!   (`version_check_release_tests`)
//! - The installed marker a version is read from (`version_check_tests`)
//! - Any shared harness (`version_check_test_support`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds. Every case here is pure:
//! no socket, no file, no clock, so the whole file runs in any order, in
//! parallel, on any machine.
//!
//! Keep this module focused on the version text and its ordering.

use super::Version;

/// A leading `v` is what a release tag carries, and the normalized form drops it.
#[test]
fn a_leading_v_is_accepted_and_never_rendered() {
    let parsed = Version::parse("v1.2.3").expect("a tagged version parses");
    assert_eq!(parsed.to_string(), "1.2.3");
    assert_eq!(Version::parse("1.2.3"), Some(parsed));
}

/// Equal versions are equal, and the ordering follows the three parts.
#[test]
fn versions_order_by_their_three_parts() {
    let older = Version::parse("1.2.3").expect("parses");
    assert_eq!(older, Version::parse("v1.2.3").expect("parses"));
    assert!(older < Version::parse("1.2.4").expect("parses"));
    assert!(older < Version::parse("1.3.0").expect("parses"));
    assert!(older < Version::parse("2.0.0").expect("parses"));
    assert!(Version::parse("v2.0.0").expect("parses") > older);
}

/// Anything that is not exactly three numeric parts is refused, so an invalid
/// tag can never be compared as if it were a version.
#[test]
fn malformed_versions_are_refused() {
    for text in [
        "",
        "v",
        "1",
        "1.2",
        "1.2.3.4",
        "1.2.3-beta.1",
        "v1.2.3-rc1",
        "1.2.3+build.5",
        "1.2.x",
        " 1.2.3",
        "1.2.3 ",
        "1..3",
        "01.2.3.4",
        "99999999999999999999999.1.1",
        "release-1.2.3",
    ] {
        assert_eq!(Version::parse(text), None, "`{text}` is not a version");
    }
}
