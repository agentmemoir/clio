// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the one release-metadata request the check makes: the
//! request against a loopback listener, and every way that answer is refused.
//!
//! ## Owns
//! - One request and the tag its body states
//! - A redirect, a stall, a non-200 status, an oversized body, and malformed
//!   metadata
//!
//! ## Does not own
//! - The comparison an established answer drives, the marker cases, and the flag
//!   refusals (`version_check_tests`)
//! - The version text and its ordering (`version_check_semver_tests`)
//! - The shared listener and reply builders (`version_check_test_support`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds. Every endpoint is a
//! listener this test owns on port 0, so no test reaches the real network, and
//! every listener is stopped and joined before the assertion that reads it.
//!
//! Keep this module focused on the release answer.

use super::{MAX_RESPONSE_BYTES, latest_release};
use crate::version_check::test_support::{
    Reply, Server, json_reply, oversized_reply, redirect_reply, status_reply,
};
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
    let server = Server::start(Reply::Bytes(oversized_reply()));
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
