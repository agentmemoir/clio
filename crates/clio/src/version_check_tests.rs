// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `version` verb's check itself: which roots the
//! installation evidence is read from, the comparison an established answer
//! drives, the unavailable line, and the flags the verb refuses.
//!
//! ## Owns
//! - The root-selection and evidence cases: portable default, harness roots,
//!   missing marker, disagreeing markers, an unknown harness refused as install's
//!   usage error, a marker reached through a symbolic link
//! - The evidence cases that must make no request at all
//! - The comparison cases: current and newer, the line each one reports, and the
//!   exit code it maps to
//! - The unavailable JSON line, and the flag refusals
//!
//! ## Does not own
//! - The release request and every way its answer is refused
//!   (`version_check_release_tests`)
//! - The version text and its ordering (`version_check_semver_tests`)
//! - The shared scratch home and listener (`version_check_test_support`)
//! - The process boundary (`version_check_harness`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds. Every endpoint these tests
//! pass is a listener this test owns on port 0, so nothing in this module reaches
//! the real network. The process suite (`version_check_harness`) cannot name an
//! endpoint at all, so it runs only over a home holding no marker, which makes no
//! request.
//!
//! Keep this module focused on the check's evidence, comparison, and flags.

use super::{Missing, ReleaseStatus, check, harness_usage_error, installation_evidence, version};
use crate::install_cli_help::supported_hint;
use crate::skill_marker::SKILL_MARKER_FILE;
use crate::version_check::test_support::{
    KEYS, Reply, Scratch, Server, env, json_reply, keys_of, package, write_marker,
};
use std::process::ExitCode;

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

/// A marker reached through a symbolic link is not evidence: the link can name a
/// file the package does not hold, so the check refuses it before reading, asks
/// nothing, and reports unavailable with nulls.
#[cfg(unix)]
#[test]
fn a_symlinked_marker_is_unavailable_without_a_request() {
    let scratch = Scratch::new("symlink-marker");
    let package = package(scratch.home(), &[".agents", "skills"]);
    std::fs::create_dir_all(&package).expect("the package directory can be created");
    let foreign = scratch.home().join("foreign");
    write_marker(&foreign, "9.9.9", "foreign");
    std::os::unix::fs::symlink(
        foreign.join(SKILL_MARKER_FILE),
        package.join(SKILL_MARKER_FILE),
    )
    .expect("the marker can be linked");
    assert_eq!(
        installation_evidence(None, &env(scratch.home())),
        Err(Missing::UnreadableMarker)
    );
    let server = Server::start(Reply::Bytes(json_reply("{\"tag_name\":\"v9.9.9\"}")));
    let report = check(None, &env(scratch.home()), &server.url("/latest"));
    assert_eq!(server.stop_and_count(), 0, "no request may be made");
    assert_eq!(report.release_status, ReleaseStatus::Unavailable);
    assert_eq!(
        report.line(),
        "{\"skillVersion\":null,\"catalogHash\":null,\"releaseStatus\":\"unavailable\",\"latestVersion\":null}"
    );
    assert_eq!(report.exit_code(), 1);
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

/// A name install does not accept is the same usage error install raises: the
/// verb exits 2 with install's own message and supported-harness hint, and never
/// reports a null line that would read as "nothing is installed". A name install
/// accepts is no refusal at all.
#[test]
fn an_unknown_harness_is_the_installation_usage_error() {
    for name in ["claude", "claude-code", "vs code"] {
        assert!(harness_usage_error(Some(name)).is_none(), "{name}");
    }
    assert!(
        harness_usage_error(None).is_none(),
        "no name means the portable root, not a refusal"
    );
    let error = harness_usage_error(Some("frobnicate")).expect("the name is refused");
    assert_eq!(error.message(), "unknown harness `frobnicate`");
    let hint = supported_hint();
    assert_eq!(error.hint(), Some(hint.as_str()));
    assert!(
        hint.starts_with("supported harnesses: ") && hint.contains("claude"),
        "{hint}"
    );
    assert_eq!(error.exit_code(), ExitCode::from(2));
    assert_eq!(
        version(&[
            "--check".to_owned(),
            "--harness".to_owned(),
            "frobnicate".to_owned(),
        ]),
        ExitCode::from(2)
    );
}

/// Missing evidence makes no request: the command never asks about a release it
/// could not compare against.
#[test]
fn missing_evidence_makes_no_request() {
    let scratch = Scratch::new("no-request");
    let server = Server::start(Reply::Bytes(json_reply("{\"tag_name\":\"v9.9.9\"}")));
    let report = check(None, &env(scratch.home()), &server.url("/latest"));
    assert_eq!(server.stop_and_count(), 0, "no request may be made");
    assert_eq!(report.release_status, ReleaseStatus::Unavailable);
    assert_eq!(report.exit_code(), 1);
}

/// Conflicting markers also make no request, and are unavailable rather than
/// current.
#[test]
fn conflicting_evidence_makes_no_request() {
    let scratch = Scratch::new("conflict-line");
    write_marker(
        &package(scratch.home(), &[".claude", "skills"]),
        "0.4.1",
        "fnv1a64:cafe",
    );
    write_marker(
        &package(scratch.home(), &[".agents", "skills"]),
        "0.5.0",
        "fnv1a64:cafe",
    );
    let server = Server::start(Reply::Bytes(json_reply("{\"tag_name\":\"v9.9.9\"}")));
    let report = check(Some("claude"), &env(scratch.home()), &server.url("/latest"));
    assert_eq!(server.stop_and_count(), 0, "no request may be made");
    assert_eq!(report.release_status, ReleaseStatus::Unavailable);
    assert_eq!(report.exit_code(), 1);
}

/// An installed version the parser refuses is unavailable, and the request is
/// skipped because there is nothing an answer could be compared with.
#[test]
fn an_unparseable_installed_version_is_unavailable_without_a_request() {
    let scratch = Scratch::new("bad-installed");
    write_marker(
        &package(scratch.home(), &[".agents", "skills"]),
        "1.2.3-beta",
        "fnv1a64:cafe",
    );
    let server = Server::start(Reply::Bytes(json_reply("{\"tag_name\":\"v2.0.0\"}")));
    let report = check(None, &env(scratch.home()), &server.url("/latest"));
    assert_eq!(server.stop_and_count(), 0);
    assert_eq!(report.skill_version.as_deref(), Some("1.2.3-beta"));
    assert_eq!(report.latest_version, None);
    assert_eq!(report.exit_code(), 1);
}

/// An installed version equal to the release is current, and the line names the
/// normalized release version rather than the tag's spelling.
#[test]
fn an_installed_version_equal_to_the_release_is_current() {
    let scratch = Scratch::new("current");
    write_marker(
        &package(scratch.home(), &[".agents", "skills"]),
        "1.2.3",
        "fnv1a64:cafe",
    );
    let server = Server::start(Reply::Bytes(json_reply("{\"tag_name\":\"v1.2.3\"}")));
    let report = check(None, &env(scratch.home()), &server.url("/latest"));
    assert_eq!(server.stop_and_count(), 1, "exactly one request is made");
    assert_eq!(
        report.line(),
        "{\"skillVersion\":\"1.2.3\",\"catalogHash\":\"fnv1a64:cafe\",\
         \"releaseStatus\":\"current\",\"latestVersion\":\"1.2.3\"}"
    );
    assert_eq!(keys_of(&report.line()), KEYS);
    assert_eq!(report.exit_code(), 0);
}

/// An installed version below the release is newer-available, and it succeeds
/// too: a newer release is an answer, not a failure.
#[test]
fn an_older_installed_version_reports_a_newer_release() {
    let scratch = Scratch::new("newer");
    write_marker(
        &package(scratch.home(), &[".agents", "skills"]),
        "1.2.3",
        "fnv1a64:cafe",
    );
    let server = Server::start(Reply::Bytes(json_reply("{\"tag_name\":\"2.0.0\"}")));
    let report = check(None, &env(scratch.home()), &server.url("/latest"));
    assert_eq!(server.stop_and_count(), 1);
    assert_eq!(report.release_status, ReleaseStatus::Newer);
    assert_eq!(report.latest_version.as_deref(), Some("2.0.0"));
    assert_eq!(report.exit_code(), 0);
}

/// An installed version above the release is current: a release that is not
/// newer is no newer release.
#[test]
fn an_installed_version_above_the_release_is_current() {
    let scratch = Scratch::new("ahead");
    write_marker(
        &package(scratch.home(), &[".agents", "skills"]),
        "3.0.0",
        "fnv1a64:cafe",
    );
    let server = Server::start(Reply::Bytes(json_reply("{\"tag_name\":\"v2.9.9\"}")));
    let report = check(None, &env(scratch.home()), &server.url("/latest"));
    assert_eq!(server.stop_and_count(), 1);
    assert_eq!(report.release_status, ReleaseStatus::Current);
    assert_eq!(report.exit_code(), 0);
}

/// A blank recorded catalog hash is unknown evidence, exactly like a blank
/// recorded version: the run cannot state one of the two facts it reports, so it
/// answers unavailable with nulls and exits 1, and it asks nothing. A hash that
/// is recorded is reported as recorded, which the current-version case holds.
#[test]
fn a_blank_recorded_catalog_hash_is_unavailable() {
    for recorded in ["", "   "] {
        let scratch = Scratch::new("blank-hash");
        write_marker(
            &package(scratch.home(), &[".agents", "skills"]),
            "1.2.3",
            recorded,
        );
        assert_eq!(
            installation_evidence(None, &env(scratch.home())),
            Err(Missing::UnreadableMarker),
            "{recorded:?}"
        );
        let server = Server::start(Reply::Bytes(json_reply("{\"tag_name\":\"v1.2.3\"}")));
        let report = check(None, &env(scratch.home()), &server.url("/latest"));
        assert_eq!(server.stop_and_count(), 0, "no request may be made");
        assert_eq!(
            report.line(),
            "{\"skillVersion\":null,\"catalogHash\":null,\
             \"releaseStatus\":\"unavailable\",\"latestVersion\":null}",
            "{recorded:?}"
        );
        assert_eq!(keys_of(&report.line()), KEYS);
        assert_eq!(report.exit_code(), 1, "{recorded:?}");
    }
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
