// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the release answer the check reads, and the comparison it
//! drives: the one request against a loopback listener, every way that answer is
//! refused, and the two statuses an established answer produces.
//!
//! ## Owns
//! - One request, a redirect, a stall, a non-200 status, an oversized body, and
//!   malformed metadata
//! - The current and newer answers, the request they each make, and their exit
//!   code
//! - The evidence cases that must make no request at all
//!
//! ## Does not own
//! - The marker and flag cases, and the version text (`version_check_tests`)
//! - The shared scratch home and listener (`version_check_test_support`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds. Every endpoint is a
//! listener this test owns on port 0, so no test reaches the real network.
//!
//! Keep this module focused on the release answer.

use super::ReleaseStatus;
use super::check;
use super::test_support::{
    KEYS, Reply, Scratch, Server, env, json_reply, keys_of, package, redirect_reply, status_reply,
    write_marker,
};
use crate::version_check_release::{MAX_RESPONSE_BYTES, latest_release};
use std::time::{Duration, Instant};

/// One request leaves the process, and its tag is read.
#[test]
fn one_request_is_made_and_the_body_is_read() {
    let server = Server::start(Reply::Bytes(json_reply("{\"tag_name\":\"v7.8.9\"}")));
    let released = latest_release(&server.url("/latest"));
    assert_eq!(server.stop_and_count(), 1, "exactly one request is made");
    assert_eq!(
        released.map(|version| version.to_string()),
        Some("7.8.9".to_owned())
    );
}

/// A redirect is not followed, so the request count stays at one.
#[test]
fn a_redirect_is_not_followed() {
    let server = Server::start(Reply::Bytes(redirect_reply()));
    let released = latest_release(&server.url("/latest"));
    assert_eq!(released, None);
    assert_eq!(server.stop_and_count(), 1, "a redirect is not followed");
}

/// A stalled answer is cut off, and the connection was made, so the bound is
/// what ended it rather than a refusal to connect.
#[test]
fn a_stalled_endpoint_is_cut_off_within_the_total_bound() {
    let server = Server::start(Reply::Stall);
    let began = Instant::now();
    let released = latest_release(&server.url("/latest"));
    let elapsed = began.elapsed();
    assert_eq!(released, None);
    assert_eq!(server.stop_and_count(), 1, "the connection was made");
    assert!(
        elapsed >= Duration::from_secs(1),
        "the deadline, not a refusal, ended it: {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_secs(30),
        "the stall must be cut off, not waited out: {elapsed:?}"
    );
}

/// Every status other than 200 is unavailable, the rate limits included.
#[test]
fn a_non_200_status_is_unavailable() {
    for code in [403, 429, 404, 500, 302] {
        let server = Server::start(Reply::Bytes(status_reply(code)));
        let released = latest_release(&server.url("/latest"));
        assert_eq!(released, None, "status {code} is unavailable");
        assert_eq!(
            server.stop_and_count(),
            1,
            "status {code} takes one request"
        );
    }
}

/// A body past the cap is refused rather than buffered.
#[test]
fn an_oversized_body_is_unavailable() {
    let server = Server::start(Reply::Bytes(super::test_support::oversized_reply()));
    let released = latest_release(&server.url("/latest"));
    assert_eq!(released, None);
    assert_eq!(server.stop_and_count(), 1);
    assert_eq!(
        MAX_RESPONSE_BYTES, 1_000_000,
        "the cap is the documented one"
    );
}

/// Metadata that is not JSON, states no tag, or states a tag that is not a
/// version is unavailable.
#[test]
fn malformed_metadata_is_unavailable() {
    for body in [
        "not json at all",
        "{\"tag_name\":null}",
        "{\"name\":\"v1.2.3\"}",
        "{\"tag_name\":\"release-1.2.3\"}",
        "{\"tag_name\":\"v1.2\"}",
        "[]",
    ] {
        let server = Server::start(Reply::Bytes(json_reply(body)));
        let released = latest_release(&server.url("/latest"));
        assert_eq!(released, None, "`{body}` states no version");
        assert_eq!(server.stop_and_count(), 1, "`{body}` takes one request");
    }
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

/// An empty recorded catalog hash is unknown, so it is reported as null.
#[test]
fn an_empty_recorded_catalog_hash_is_null() {
    let scratch = Scratch::new("empty-hash");
    write_marker(
        &package(scratch.home(), &[".agents", "skills"]),
        "1.2.3",
        "",
    );
    let server = Server::start(Reply::Bytes(json_reply("{\"tag_name\":\"v1.2.3\"}")));
    let report = check(None, &env(scratch.home()), &server.url("/latest"));
    assert_eq!(server.stop_and_count(), 1);
    assert_eq!(report.catalog_hash, None);
    assert_eq!(report.release_status, ReleaseStatus::Current);
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
