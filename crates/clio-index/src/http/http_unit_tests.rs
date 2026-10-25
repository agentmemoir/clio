// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for unit tests of the JSON HTTP transport
//! internals in `http.rs`: endpoint parsing, one-shot agent CA configuration,
//! error classification, and response parsing.
//!
//! ## Owns
//! - Endpoint parse acceptance and fail-closed cases (schemes, ports, IPv6 brackets)
//! - Custom root CA configuration failure (`agent` with a malformed PEM)
//! - `transport_error` retryability classification, including rustls
//!   certificate-verification failures that arrive as I/O errors
//! - `parse_response_json` status/snippet handling and 401/403 secret hiding
//!
//! ## Does not own
//! - Live sidecar round trips and retry behavior (see `http_tests`)
//! - Verified-TLS mock server tests (see `tls_mock_tests`)
//!
//! ## Boundary
//!
//! Changes to transport internals should normally update tests here.
//! Changes to on-the-wire transport behavior should not require changes here.
//!
//! Keep this module focused on internal unit tests for `http`.

use super::*;
use crate::embed::is_retryable;

#[test]
fn endpoint_parse_accepts_http_and_https() {
    let e = HttpEndpoint::parse("http://127.0.0.1:34311").unwrap();
    assert_eq!(
        (e.host.as_str(), e.port, e.path.as_str()),
        ("127.0.0.1", 34311, "/")
    );
    let e = HttpEndpoint::parse("https://secure.example.com/embed").unwrap();
    assert_eq!((e.port, e.path.as_str()), (443, "/embed"));
    let e = HttpEndpoint::parse("http://host.example.com").unwrap();
    assert_eq!(e.port, 80);
}

#[test]
fn invalid_ca_pem_fails_fast() {
    let err = agent(Duration::from_secs(1), Some(b"not-a-pem")).unwrap_err();
    assert!(err.to_string().contains("tls config"), "{err}");
}

#[test]
fn endpoint_parse_fails_closed() {
    for bad in ["ftp://x", "", "http://", "https://", "http://:notaport"] {
        assert!(HttpEndpoint::parse(bad).is_err(), "{bad}");
    }
    let e = HttpEndpoint::parse("http://h:99999").unwrap_err();
    assert!(e.to_string().contains("invalid port"), "{e}");
}

#[test]
fn ureq_error_mapping_classifies_retryability() {
    let err = transport_error("ctx", ureq::Error::StatusCode(503));
    assert!(err.to_string().contains("HTTP 503"), "{err}");
    assert!(is_retryable(&err));

    let err = transport_error("ctx", ureq::Error::StatusCode(404));
    assert!(err.to_string().contains("HTTP 404"), "{err}");
    assert!(!is_retryable(&err));

    for e in [ureq::Error::ConnectionFailed, ureq::Error::HostNotFound] {
        let err = transport_error("ctx", e);
        let text = err.to_string();
        assert!(
            text.contains("sidecar unreachable") && text.contains("connect failed"),
            "{text}"
        );
        assert!(is_retryable(&err));
    }

    let err = transport_error("ctx", ureq::Error::Io(std::io::Error::other("boom")));
    assert!(err.to_string().contains("connect failed"), "{err}");
    assert!(is_retryable(&err));

    // rustls verification failures arrive as Io with the rustls text, which
    // contains "invalid peer certificate"; they must not be retryable.
    let err = transport_error(
        "ctx",
        ureq::Error::Io(std::io::Error::other(
            "invalid peer certificate: UnknownIssuer",
        )),
    );
    assert!(err.to_string().contains("tls failure"), "{err}");
    assert!(!is_retryable(&err), "{err}");

    // Terminal client errors (Tls, BadUri): no "sidecar unreachable"
    // marker and no retryable substring, so is_retryable is false.
    // Protocol(ureq_proto::Error) maps the same way but the inner error
    // type is not constructible here (ureq does not re-export ureq_proto).
    for e in [
        ureq::Error::BadUri("bad".to_owned()),
        ureq::Error::Tls("no tls"),
    ] {
        let err = transport_error("ctx", e);
        let text = err.to_string();
        assert!(
            !text.contains("sidecar unreachable")
                && !text.contains("connect failed")
                && !text.contains("read timed out"),
            "{text}"
        );
        assert!(!is_retryable(&err), "{text}");
    }
}

#[test]
fn endpoint_parse_accepts_bracketed_ipv6() {
    let e = HttpEndpoint::parse("http://[::1]").unwrap();
    assert_eq!((e.host.as_str(), e.port, e.path.as_str()), ("::1", 80, "/"));
    let e = HttpEndpoint::parse("https://[::1]").unwrap();
    assert_eq!((e.host.as_str(), e.port), ("::1", 443));
    let e = HttpEndpoint::parse("http://[::1]:8080/embed").unwrap();
    assert_eq!(
        (e.host.as_str(), e.port, e.path.as_str()),
        ("::1", 8080, "/embed")
    );
}

#[test]
fn endpoint_parse_fails_closed_on_malformed_brackets() {
    for bad in [
        "http://[::1",
        "http://[:x]",
        "http://[::1]junk",
        "http://[::1]:notaport",
    ] {
        assert!(HttpEndpoint::parse(bad).is_err(), "{bad}");
    }
    let e = HttpEndpoint::parse("http://[::1").unwrap_err();
    assert!(e.to_string().contains("bracketed host"), "{e}");
}

#[test]
fn parse_response_json_hides_body_for_401_403() {
    let secret_body = b"token abc123 must not leak";
    for code in [401_u16, 403] {
        let err = parse_response_json(code, secret_body).unwrap_err();
        let text = err.to_string();
        assert!(text.contains(&format!("HTTP {code}")), "{text}");
        assert!(!text.contains("abc123"), "{text}");
    }
    let err = parse_response_json(503, secret_body).unwrap_err();
    assert!(err.to_string().contains("abc123"), "{err}");
}

#[test]
fn parse_response_json_semantics() {
    assert!(parse_response_json(200, b"[[0.1]]").unwrap().is_array());
    let err = parse_response_json(503, b"busy right now").unwrap_err();
    assert!(err.to_string().contains("HTTP 503"), "{err}");
    assert!(err.to_string().contains("busy"), "{err}");
    let err = parse_response_json(200, b"not-json").unwrap_err();
    assert!(err.to_string().contains("non-JSON body"), "{err}");
}
