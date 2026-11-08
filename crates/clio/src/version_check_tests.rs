// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the parts of the `version` verb's check that need no
//! socket: which roots the installation evidence is read from, the version
//! text's grammar and ordering, the unavailable line, and the flags the verb
//! refuses.
//!
//! ## Owns
//! - The root-selection and evidence cases: portable default, harness roots,
//!   missing marker, disagreeing markers, unknown harness
//! - The version parser and comparison cases
//! - The unavailable JSON line, and the flag refusals
//!
//! ## Does not own
//! - Anything that reads a release answer (`version_check_http_tests`)
//! - The shared scratch home and listener (`version_check_test_support`)
//! - The process boundary (`version_check_harness`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the check's evidence and flags.

use super::test_support::{KEYS, Scratch, env, keys_of, package, write_marker};
use super::{Missing, check, installation_evidence, version};
use crate::version_check_semver::Version;
use std::process::ExitCode;

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

/// With no `--harness` the portable root is the only root read, which is what
/// `clio install` writes with no harness.
#[test]
fn the_portable_root_is_read_with_no_harness() {
    let scratch = Scratch::new("portable");
    write_marker(
        &package(scratch.home(), &[".agents", "skills"]),
        "0.4.1",
        "fnv1a64:cafe",
    );
    let evidence = installation_evidence(None, &env(scratch.home())).expect("the marker is read");
    assert_eq!(evidence.package_version, "0.4.1");
    assert_eq!(evidence.catalog_hash, "fnv1a64:cafe");
}

/// A named harness reads the same roots `install --harness` writes: the native
/// root first, the portable root beside it.
#[test]
fn a_harness_reads_its_own_roots_and_records_an_alias() {
    let scratch = Scratch::new("harness");
    write_marker(
        &package(scratch.home(), &[".claude", "skills"]),
        "0.4.1",
        "fnv1a64:cafe",
    );
    for name in ["claude", "claude-code"] {
        let evidence =
            installation_evidence(Some(name), &env(scratch.home())).expect("the native marker");
        assert_eq!(evidence.package_version, "0.4.1", "{name}");
    }
    // A root another harness reads is not one `claude` reads.
    let other = Scratch::new("harness-other");
    write_marker(
        &package(other.home(), &[".gemini", "config", "skills"]),
        "0.4.1",
        "fnv1a64:cafe",
    );
    assert_eq!(
        installation_evidence(Some("claude"), &env(other.home())),
        Err(Missing::NoMarker)
    );
}

/// No marker anywhere is missing evidence, not a current installation.
#[test]
fn a_missing_marker_is_missing_evidence() {
    let scratch = Scratch::new("missing");
    std::fs::create_dir_all(package(scratch.home(), &[".agents", "skills"]))
        .expect("an empty package directory");
    assert_eq!(
        installation_evidence(None, &env(scratch.home())),
        Err(Missing::NoMarker)
    );
}

/// Two roots that disagree are conflicting evidence; two that agree are used.
#[test]
fn disagreeing_markers_are_conflicting_evidence() {
    let scratch = Scratch::new("conflict");
    write_marker(
        &package(scratch.home(), &[".claude", "skills"]),
        "0.4.1",
        "fnv1a64:cafe",
    );
    write_marker(
        &package(scratch.home(), &[".agents", "skills"]),
        "0.4.1",
        "fnv1a64:cafe",
    );
    let agreeing = installation_evidence(Some("claude"), &env(scratch.home()))
        .expect("agreeing markers are used");
    assert_eq!(agreeing.package_version, "0.4.1");

    write_marker(
        &package(scratch.home(), &[".claude", "skills"]),
        "0.5.0",
        "fnv1a64:cafe",
    );
    assert_eq!(
        installation_evidence(Some("claude"), &env(scratch.home())),
        Err(Missing::ConflictingMarkers)
    );
    write_marker(
        &package(scratch.home(), &[".claude", "skills"]),
        "0.4.1",
        "fnv1a64:beef",
    );
    assert_eq!(
        installation_evidence(Some("claude"), &env(scratch.home())),
        Err(Missing::ConflictingMarkers)
    );
}

/// A name install does not accept selects no root at all, so the check has
/// nothing to read.
#[test]
fn an_unknown_harness_selects_no_root() {
    let scratch = Scratch::new("unknown");
    write_marker(
        &package(scratch.home(), &[".agents", "skills"]),
        "0.4.1",
        "fnv1a64:cafe",
    );
    assert_eq!(
        installation_evidence(Some("frobnicate"), &env(scratch.home())),
        Err(Missing::UnknownHarness)
    );
}

/// The unavailable line states four keys, in order, and every one of them is
/// null or `unavailable`: it never claims `current`.
#[test]
fn the_unavailable_line_carries_four_keys_and_nulls() {
    let scratch = Scratch::new("unavailable-line");
    let report = check(None, &env(scratch.home()), "http://127.0.0.1:1/unused");
    assert_eq!(
        report.line(),
        "{\"skillVersion\":null,\"catalogHash\":null,\"releaseStatus\":\"unavailable\",\"latestVersion\":null}"
    );
    assert_eq!(keys_of(&report.line()), KEYS);
    assert_eq!(report.exit_code(), 1);
}

/// `--harness` without `--check` is a usage error, and an unknown flag on the
/// verb is a usage error too, rather than a silently ignored token.
#[test]
fn an_unpaired_or_unknown_flag_is_a_usage_error() {
    assert_eq!(
        version(&["--harness".to_owned(), "claude".to_owned()]),
        ExitCode::from(2)
    );
    assert_eq!(version(&["--frobnicate".to_owned()]), ExitCode::from(2));
}
