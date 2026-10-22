// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for two things the transport now refuses before a
//! socket is opened: a remote plain-`http` endpoint, and a URL that smuggles a
//! trusted-looking prefix in front of a real remote host.
//!
//! ## Owns
//! - The refusal message an operator reads, on every send function
//! - Proof that the refusal happens before any connection is made
//! - Proof that a loopback endpoint still works
//!
//! ## Does not own
//! - The transport itself, or its retry classification (`http_tests`)
//! - The config-time rule that stops the same value earlier (`clio-config`)
//! - Certificate verification (`tls_mock_tests`)
//!
//! ## Boundary
//!
//! Changes to the request-time gate or the probe should normally require
//! changes here. Changes to how a request is sent should not require changes
//! here.

use crate::embed::is_retryable;
use crate::embed_test_support::FakeEmbedServer;
use crate::http::{get_json, get_ok, post_json};
use crate::probe::endpoint;
use clio_types::ErrorCode;
use serde_json::json;
use std::net::TcpListener;
use std::time::Duration;

const FAST: Duration = Duration::from_millis(2000);

/// A URL on this machine that nothing is listening on, used where a test needs
/// a loopback address that cannot answer.
fn dead_loopback_url() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("probe bind");
    let port = listener.local_addr().expect("addr").port();
    drop(listener);
    format!("http://127.0.0.1:{port}")
}

#[test]
fn a_remote_plain_http_request_is_refused_before_any_socket_opens() {
    // Nothing is listening on this address and no DNS lookup can succeed, so the
    // only way this can fail is by being refused up front. A transport error
    // would mean the gate let it through and the request went out.
    for url in [
        "http://api.example.com/embed",
        "http://api.example.com:8080/embed",
        "http://192.0.2.1/embed",
    ] {
        let err = post_json(url, &json!({"inputs": ["a"]}), FAST, None)
            .expect_err("a remote plain-http endpoint must be refused");
        assert_eq!(err.code, ErrorCode::ConfigCorrupt, "{url}");
        let text = err.to_string();
        assert!(text.contains("must be https"), "{url}: {text}");
        assert!(text.contains("unencrypted"), "{url}: {text}");
        // A refused send is not something a retry can fix.
        assert!(!is_retryable(&err), "{url}: {text}");
    }
}

#[test]
fn a_smuggled_trusted_prefix_does_not_reach_the_real_host() {
    // The parse gives the host `trusted@evil.example.com` and the port, and the gate
    // asks whether *that* host is this machine. A split of the string on `://`
    // and `/` would read `trusted` and wave this through.
    for url in [
        "http://trusted@evil.example.com/embed",
        "http://localhost@evil.example.com/embed",
        "http://127.0.0.1@evil.example.com/embed",
        "http://box.localhost@evil.example.com/embed",
    ] {
        let err = post_json(url, &json!({}), FAST, None)
            .expect_err("a userinfo prefix must not stand in for the host");
        let text = err.to_string();
        assert!(text.contains("must be https"), "{url}: {text}");
    }
}

#[test]
fn a_loopback_endpoint_still_reaches_the_sidecar() {
    let server = FakeEmbedServer::spawn();
    // The local sidecars are on this machine, so plain http is correct for them
    // and the gate must not stand in the way. The mock binds 127.0.0.1 only, so
    // this is the one loopback spelling that can be dialled here; the others are
    // covered by the gate assertion below.
    let value = post_json(&server.url, &json!({"inputs": ["a"]}), FAST, None)
        .expect("a loopback sidecar must still be reachable over plain http");
    assert!(
        value.is_array(),
        "the mock returns a bare vector array: {value}"
    );
    get_ok(&server.url, FAST, None).expect("a loopback health check over http");
}

#[test]
fn every_loopback_spelling_passes_the_gate() {
    // The rule is the host, not the address family or the spelling, so each of
    // these is asked of the gate directly rather than dialled.
    for url in [
        "http://localhost:34311/embed",
        "http://LOCALHOST:34311/embed",
        "http://box.localhost:34311/embed",
        "http://127.0.0.1:34311/embed",
        "http://127.1.2.3:34311/embed",
        "http://[::1]:34311/embed",
    ] {
        crate::http::gate(url).unwrap_or_else(|e| panic!("{url} must pass the gate: {e}"));
    }
}

#[test]
fn a_https_endpoint_is_not_refused_for_its_scheme() {
    // A verified-TLS mock is `tls_mock_tests`' job. What matters here is that
    // the gate reads the scheme and lets `https` through to the transport, which
    // then reports the real failure. A gate error would say `must be https` for
    // a URL that already is.
    let url = dead_loopback_url().replacen("http", "https", 1);
    let err = post_json(&url, &json!({}), FAST, None).expect_err("nothing is listening");
    assert!(
        !err.to_string().contains("must be https"),
        "an https endpoint must pass the gate: {err}"
    );
}

#[test]
fn every_send_function_goes_through_the_gate() {
    // Three send functions, three refusals. A new send path that forgets the
    // gate is the failure this test exists to make visible.
    let err = get_ok("http://api.example.com/health", FAST, None).expect_err("get_ok");
    assert!(err.to_string().contains("must be https"), "{err}");
    let err = get_json("http://api.example.com/v1", FAST, None).expect_err("get_json");
    assert!(err.to_string().contains("must be https"), "{err}");
    let err =
        post_json("http://api.example.com/embed", &json!({}), FAST, None).expect_err("post_json");
    assert!(err.to_string().contains("must be https"), "{err}");
}

#[test]
fn the_probe_reports_that_a_live_endpoint_answered() {
    let server = FakeEmbedServer::spawn();
    let answer = endpoint(&server.url, None).expect("a running sidecar answers");
    assert_eq!(answer, "answered");
}

#[test]
fn the_probe_sends_the_key_and_reports_a_refusal_for_a_remote_http_endpoint() {
    let server = FakeEmbedServer::spawn();
    // The key the operator typed is on the wire, and the probe sends it the way
    // the real request path does.
    let answer = endpoint(&server.url, Some("tok-12345678901234567890"))
        .expect("a running sidecar answers with a key");
    assert_eq!(answer, "answered");
    assert!(
        server
            .last_auth()
            .is_some_and(|auth| auth.starts_with("Bearer tok-"))
    );

    // A remote plain-http endpoint is stopped before the key is sent anywhere.
    let err = endpoint("http://api.example.com/embed", Some("secret-key"))
        .expect_err("the gate refuses it");
    assert!(err.to_string().contains("must be https"), "{err}");
    assert!(
        !err.to_string().contains("secret-key"),
        "the key must not appear in the error: {err}"
    );
}

#[test]
fn the_probe_reports_the_transports_own_failure() {
    // A dead loopback endpoint passes the gate and then fails in the transport.
    // Reporting that failure as it is, rather than as a summary, is the point of
    // the probe living here.
    let err = endpoint(&dead_loopback_url(), None).expect_err("nothing is listening");
    assert!(!err.to_string().contains("must be https"), "{err}");
    assert!(is_retryable(&err), "{err}");
}
