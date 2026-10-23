// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the ureq-based sidecar transport.
//!
//! ## Owns
//! - URL parsing (http/https, default ports), round trips, bearer headers, health probes
//! - Bounded-timeout, error-status, oversize-body, non-JSON, and dead-endpoint cases
//! - Retry-classification assertions against `embed::is_retryable`
//!
//! ## Does not own
//! - Embed-text assembly or dim gating (see `embed_tests`)
//! - Worker/rebuild behavior (see `worker_tests`/`rebuild_tests`)
//!
//! ## Boundary
//!
//! Changes to transport behavior should normally update tests here.
//! Changes to embedding semantics should not require changes here.
//!
//! Keep this module focused on HTTP transport behavior.

use crate::embed::is_retryable;
use crate::embed_test_support::{FakeEmbedServer, dead_url};
use crate::http::{get_json, get_ok, post_json};
use serde_json::json;
use std::time::Duration;

const FAST: Duration = Duration::from_millis(2000);

#[test]
fn parse_http_url_shapes() {
    let e = crate::http::HttpEndpoint::parse("http://127.0.0.1:34311").unwrap();
    assert_eq!(e.host, "127.0.0.1");
    assert_eq!(e.port, 34311);
    assert_eq!(e.path, "/");
    let e = crate::http::HttpEndpoint::parse("http://host.example.com/embed").unwrap();
    assert_eq!(e.port, 80);
    assert_eq!(e.path, "/embed");
    let e = crate::http::HttpEndpoint::parse("https://secure.example.com/embed").unwrap();
    assert_eq!(e.port, 443);
    assert_eq!(e.path, "/embed");
    assert!(crate::http::HttpEndpoint::parse("ftp://x").is_err());
    assert!(crate::http::HttpEndpoint::parse("http://").is_err());
    assert!(crate::http::HttpEndpoint::parse("https://").is_err());
    assert!(crate::http::HttpEndpoint::parse("http://:notaport").is_err());
}

#[test]
fn post_json_roundtrip_and_get_health() {
    let server = FakeEmbedServer::spawn();
    let value = post_json(&server.url, &json!({"inputs": ["a"]}), FAST, None).unwrap();
    let arr = value.as_array().expect("vector array");
    assert_eq!(arr.len(), 1);
    get_ok(&server.url, FAST, None).unwrap();
}

#[test]
fn bearer_header_reaches_sidecar() {
    let server = FakeEmbedServer::spawn();
    post_json(
        &server.url,
        &json!({"inputs": ["a"]}),
        FAST,
        Some("tok-12345678901234567890"),
    )
    .unwrap();
    assert!(
        server
            .last_auth()
            .expect("auth header")
            .starts_with("Bearer tok-")
    );
}

#[test]
fn connection_refused_is_retryable() {
    let url = dead_url();
    let err = post_json(&url, &json!({}), Duration::from_millis(500), None).unwrap_err();
    let text = err.to_string();
    assert!(
        text.contains("connect failed") || text.contains("sidecar unreachable"),
        "{text}"
    );
    assert!(is_retryable(&err), "{text}");
    let err = get_ok(&url, Duration::from_millis(500), None).unwrap_err();
    assert!(err.to_string().contains("sidecar unreachable"), "{err}");
    assert!(is_retryable(&err), "{err}");
    assert!(crate::http::HttpEndpoint::parse(&url).is_ok());
}

#[test]
fn slow_response_times_out_and_is_retryable() {
    // 300ms server delay > 150ms client timeout, but Drop joins quickly.
    let server = FakeEmbedServer::spawn_slow(Duration::from_millis(300));
    let err = post_json(
        &server.url,
        &json!({"inputs": ["a"]}),
        Duration::from_millis(150),
        None,
    )
    .unwrap_err();
    let text = err.to_string();
    assert!(
        text.contains("timed out") || text.contains("deadline") || text.contains("sidecar"),
        "{text}"
    );
    assert!(is_retryable(&err), "{text}");
}

#[test]
fn error_statuses_classify_retryability() {
    let server = FakeEmbedServer::spawn_status(503);
    let err = post_json(&server.url, &json!({"inputs": ["a"]}), FAST, None).unwrap_err();
    assert!(err.to_string().contains("HTTP 503"), "{err}");
    assert!(is_retryable(&err));
    drop(server);

    let server = FakeEmbedServer::spawn_status(429);
    let err = post_json(&server.url, &json!({"inputs": ["a"]}), FAST, None).unwrap_err();
    assert!(err.to_string().contains("HTTP 429"), "{err}");
    assert!(is_retryable(&err));
    drop(server);

    let server = FakeEmbedServer::spawn_status(404);
    let err = post_json(&server.url, &json!({"inputs": ["a"]}), FAST, None).unwrap_err();
    assert!(err.to_string().contains("HTTP 404"), "{err}");
    assert!(!is_retryable(&err));
}

#[test]
fn get_ok_rejects_4xx() {
    let server = FakeEmbedServer::spawn_status(404);
    let err = get_ok(&server.url, FAST, None).unwrap_err();
    assert!(err.to_string().contains("HTTP 404"), "{err}");
    assert!(!is_retryable(&err));
}

#[test]
fn oversize_response_hits_safety_cap() {
    // Just over the 64 MiB read cap — enough to trigger it, not 1 MiB more.
    let cap_plus_1 = 64 * 1024 * 1024 + 1;
    let mut raw = b"HTTP/1.1 200 OK\r\nContent-Length: ".to_vec();
    raw.extend_from_slice(cap_plus_1.to_string().as_bytes());
    raw.extend_from_slice(b"\r\n\r\n");
    raw.extend(std::iter::repeat_n(b'x', cap_plus_1));
    let server = FakeEmbedServer::spawn_raw(raw);
    let err = post_json(&server.url, &json!({"inputs": ["a"]}), FAST, None).unwrap_err();
    assert!(err.to_string().contains("safety cap"), "{err}");
}

#[test]
fn non_json_body_and_malformed_response_are_reported() {
    let server = FakeEmbedServer::spawn_raw(
        b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\n\r\nnot-json".to_vec(),
    );
    let err = post_json(&server.url, &json!({"inputs": ["a"]}), FAST, None).unwrap_err();
    assert!(err.to_string().contains("non-JSON body"), "{err}");
    drop(server);
    let server = FakeEmbedServer::spawn_raw(b"junk-without-head".to_vec());
    let err = post_json(&server.url, &json!({"inputs": ["a"]}), FAST, None).unwrap_err();
    // Garbage HTTP data is a terminal protocol error: no "(sidecar
    // unreachable)" marker, so retry classification is false.
    let text = err.to_string();
    assert!(text.contains("protocol error"), "{text}");
    assert!(!text.contains("sidecar unreachable"), "{text}");
    assert!(!is_retryable(&err), "{text}");
}

#[test]
fn post_json_status_error_has_no_embed_label() {
    let server = FakeEmbedServer::spawn_status(503);
    let err = post_json(&server.url, &json!({"inputs": ["a"]}), FAST, None).unwrap_err();
    let text = err.to_string();
    assert!(text.contains("HTTP 503"), "{text}");
    assert!(!text.contains("embed"), "embed label leaked: {text}");
}

#[test]
fn get_ok_status_error_has_no_embed_label() {
    let server = FakeEmbedServer::spawn_status(503);
    let err = get_ok(&server.url, FAST, None).unwrap_err();
    let text = err.to_string();
    assert!(text.contains("HTTP 503"), "{text}");
    assert!(!text.contains("embed"), "embed label leaked: {text}");
}

#[test]
fn post_json_dead_endpoint_has_no_embed_label_and_stays_retryable() {
    let url = dead_url();
    let err = post_json(&url, &json!({}), Duration::from_millis(500), None).unwrap_err();
    let text = err.to_string();
    assert!(!text.contains("embed"), "embed label leaked: {text}");
    assert!(
        text.contains("connect failed") || text.contains("sidecar unreachable"),
        "{text}"
    );
    assert!(is_retryable(&err), "{text}");
}

#[test]
fn get_ok_dead_endpoint_has_no_embed_label_and_stays_retryable() {
    let url = dead_url();
    let err = get_ok(&url, Duration::from_millis(500), None).unwrap_err();
    let text = err.to_string();
    assert!(!text.contains("embed"), "embed label leaked: {text}");
    assert!(
        text.contains("connect failed") || text.contains("sidecar unreachable"),
        "{text}"
    );
    assert!(is_retryable(&err), "{text}");
}

#[test]
fn https_dead_endpoint_reports_transport_failure() {
    // Dead https:// endpoint exercises the TLS-capable ureq path; a verified-TLS
    // mock server would need cert generation deps that are out of scope here.
    let url = dead_url().replacen("http", "https", 1);
    let err = post_json(&url, &json!({"inputs": ["a"]}), FAST, None).unwrap_err();
    let text = err.to_string();
    assert!(
        text.contains("connect failed") || text.contains("sidecar unreachable"),
        "{text}"
    );
    assert!(is_retryable(&err), "{text}");
    assert!(crate::http::HttpEndpoint::parse(&url).is_ok());
}

#[test]
fn get_json_fetches_payload_and_records_auth() {
    let raw = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n[{\"id\":\"m1\"}]\r\n".to_vec();
    let server = FakeEmbedServer::spawn_raw(raw);
    let val = get_json(&server.url, FAST, Some("Token secret123")).expect("get_json succeeds");
    assert_eq!(val, json!([{"id": "m1"}]));
    assert_eq!(server.last_auth().as_deref(), Some("Token secret123"));
}

#[test]
fn get_json_bearer_shorthand_formats_header() {
    let raw =
        b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{}\r\n"
            .to_vec();
    let server = FakeEmbedServer::spawn_raw(raw);
    let _ = get_json(&server.url, FAST, Some("bare-key")).expect("ok");
    assert_eq!(server.last_auth().as_deref(), Some("Bearer bare-key"));
}

#[test]
fn get_json_dead_endpoint_reports_transport_failure() {
    let url = dead_url();
    let err = get_json(&url, Duration::from_millis(500), None).unwrap_err();
    let text = err.to_string();
    assert!(
        text.contains("connect failed") || text.contains("sidecar unreachable"),
        "{text}"
    );
    assert!(is_retryable(&err), "{text}");
}
