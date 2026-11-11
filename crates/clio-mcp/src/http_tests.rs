// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! Streamable HTTP transport tests: the request gates, the stateless
//! admission, and discovery/tools/call end to end.

use super::http_test_support_tests::{
    admission, post_admitted, post_call, post_served, request, served_body, spawn_all_tools_server,
    spawn_server,
};
use super::*;
use crate::conn_pool::MAX_CONNECTIONS;
use crate::http_guard::origin_host_allowed;
use crate::runtime::McpOpenOptions;
use clio_config::auth_tokens::{AuthTokenRecord, digest_hex};
use clio_types::Actor;
use serde_json::{Value, json};

/// A store holding the one record whose secret is `secret-token`, built without
/// a file: the literal keeps the request fixtures below readable.
fn single_record_store() -> CredentialStore {
    CredentialStore::from_records(vec![AuthTokenRecord {
        label: "ci".into(),
        secret_sha256: digest_hex("secret-token"),
        actor: Actor::Harness,
        banks: vec!["demo".into()],
        created_at: "2026-01-01T00:00:00Z".into(),
    }])
}

/// The revision this server serves is discoverable rather than negotiated:
/// `initialize` was removed from the revision, so a client that still opens
/// with it is answered as an unknown method.
#[test]
fn t06_initialize_returns_pinned_revision() {
    let (port, _) = spawn_server(HttpConfig::default());
    let res = post_served(port, "server/discover", "");
    assert_eq!(res.status, 200);
    let body: Value = serde_json::from_str(&res.body).expect("json");
    assert_eq!(
        body["result"]["supportedVersions"],
        json!([crate::MCP_PROTOCOL_REVISION])
    );
    assert_eq!(
        body["result"]["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
        json!("clio-mcp")
    );
    let removed = post_admitted(port, "initialize", &initialize_frame(), "");
    assert_eq!(removed.status, 404, "a removed method is not served");
    let body: Value = serde_json::from_str(&removed.body).expect("json");
    assert_eq!(body["error"]["code"], json!(-32601));
}

/// A client still opening with `initialize` sends the retired handshake's
/// parameters; the body is built here so the refusal is about the method.
fn initialize_frame() -> String {
    serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {
            "_meta": crate::stateless_fixtures::required_meta(crate::MCP_PROTOCOL_REVISION),
            "protocolVersion": crate::MCP_PROTOCOL_REVISION,
            "capabilities": {}
        }
    })
    .to_string()
}

#[test]
fn t08_bad_origin_is_403() {
    let (port, _) = spawn_server(HttpConfig::default());
    let res = post_served(
        port,
        "server/discover",
        "Origin: http://evil.example.com\r\n",
    );
    assert_eq!(res.status, 403);
    let ok = post_served(port, "server/discover", "Origin: http://localhost:5173\r\n");
    assert_eq!(ok.status, 200);
}

#[test]
fn t09_missing_auth_rejected_when_a_store_record_is_configured() {
    let (port, _) = spawn_server(HttpConfig {
        credentials: Some(Arc::new(single_record_store())),
        ..HttpConfig::default()
    });
    let no_auth = post_served(port, "server/discover", "");
    assert_eq!(no_auth.status, 401);
    let wrong = post_served(port, "server/discover", "Authorization: Bearer nope\r\n");
    assert_eq!(wrong.status, 401);
    let good = post_served(
        port,
        "server/discover",
        "Authorization: Bearer secret-token\r\n",
    );
    assert_eq!(good.status, 200);
}

/// `insecure_no_auth` alone must not admit a bind outside loopback, and the
/// refusal has to name the second opt-out an operator needs to serve without a
/// credential on every interface.
#[test]
fn non_loopback_without_insecure_bind_any_is_refused() {
    let state = crate::runtime::McpState::open_with_effective(
        &McpOpenOptions::default(),
        &serde_json::json!({}),
    )
    .expect("state");
    let listener = TcpListener::bind("0.0.0.0:0").expect("bind");
    let cfg = HttpConfig {
        insecure_no_auth: true,
        ..HttpConfig::default()
    };
    let message = serve_with_listener(state, cfg, listener)
        .expect_err("insecure_no_auth alone must not admit a non-loopback bind")
        .to_string();

    assert!(
        message.contains("non-loopback bind requires an auth token"),
        "the pinned phrase must survive: {message}"
    );
    assert!(
        message.contains("--insecure-bind-any"),
        "the refusal must name the second opt-out: {message}"
    );
}

/// Both opt-outs admit the bind, so the listener serves every interface and
/// answers an unauthenticated request: what the pair is for, and the only way
/// the non-loopback case is reachable.
#[test]
fn non_loopback_with_both_opt_outs_serves_unauthenticated() {
    let state = crate::runtime::McpState::open_with_effective(
        &McpOpenOptions::default(),
        &serde_json::json!({}),
    )
    .expect("state");
    let listener = TcpListener::bind("0.0.0.0:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let cfg = HttpConfig {
        insecure_no_auth: true,
        insecure_bind_any: true,
        ..HttpConfig::default()
    };
    std::thread::spawn(move || {
        let _ = serve_with_listener(state, cfg, listener);
    });
    let res = post_served(port, "server/discover", "");
    assert_eq!(
        res.status, 200,
        "both opt-outs admit the bind: {}",
        res.body
    );
}

#[test]
fn get_sse_and_wrong_paths() {
    let (port, _) = spawn_server(HttpConfig::default());
    let get = request(port, "GET /mcp HTTP/1.1\r\nHost: localhost\r\n\r\n");
    assert_eq!(get.status, 405, "GET SSE not offered under this pin");
    let miss = post_served(port, "server/discover", "");
    assert_eq!(miss.status, 200);
    let body = served_body("server/discover");
    let wrong_path = request(
        port,
        &format!(
            "POST /other HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nAccept: application/json\r\nContent-Length: {}\r\n{}\r\n{}",
            body.len(),
            admission("server/discover"),
            body
        ),
    );
    assert_eq!(wrong_path.status, 404);
}

#[test]
fn tools_call_end_to_end_over_http() {
    let (port, _) = spawn_all_tools_server(HttpConfig::default());
    let res = post_call(
        port,
        "triple_add",
        r#"{"subject":"a","predicate":"b","object":"c","epistemic_kind":"fact","bank":"bank-a"}"#,
        "",
    );
    assert_eq!(res.status, 200, "{}", res.body);
    let body: Value = serde_json::from_str(&res.body).expect("json");
    let result = &body["result"];
    assert_eq!(result["isError"], json!(false));
    assert!(result["structuredContent"]["ok"].as_bool().unwrap_or(false));
}

#[test]
fn notification_post_returns_202() {
    let (port, _) = spawn_server(HttpConfig::default());
    // A notification carries the required metadata like any other request, and
    // is still answered with an empty 202.
    let notification = format!(
        r#"{{"jsonrpc":"2.0","method":"notifications/initialized","params":{{"_meta":{}}}}}"#,
        crate::stateless_fixtures::required_meta(crate::MCP_PROTOCOL_REVISION)
    );
    let res = post_admitted(port, "notifications/initialized", &notification, "");
    assert_eq!(res.status, 202, "{}", res.body);
    assert!(res.body.is_empty(), "{:?}", res.body);
}

#[test]
fn wrong_content_type_is_415() {
    let (port, _) = spawn_server(HttpConfig::default());
    let body = served_body("server/discover");
    let res = request(
        port,
        &format!(
            "POST /mcp HTTP/1.1\r\nHost: localhost\r\nContent-Type: text/plain\r\nAccept: application/json\r\nContent-Length: {}\r\n{}\r\n{}",
            body.len(),
            admission("server/discover"),
            body
        ),
    );
    assert_eq!(res.status, 415);
}

/// Socket-level 406 and 405, so the router branches are covered by a real
/// request rather than only by the `accepts_json` predicate in isolation. Each
/// request declares a body, so a request-line failure (400) cannot masquerade
/// as a routing refusal.
#[test]
fn unacceptable_accept_is_406_and_unserved_methods_are_405() {
    let (port, _) = spawn_server(HttpConfig::default());
    let body = served_body("server/discover");
    let with_accept = |method: &str, accept: &str| {
        request(
            port,
            &format!(
                "{method} /mcp HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\n\
                 Accept: {accept}\r\nContent-Length: {}\r\n{}\r\n{}",
                body.len(),
                admission("server/discover"),
                body
            ),
        )
    };
    // The same request with an acceptable Accept serves, so the 406 below is
    // attributable to the Accept header and not to the request shape.
    assert_eq!(with_accept("POST", "application/json").status, 200);
    assert_eq!(
        with_accept("POST", "text/html").status,
        406,
        "text/html refused"
    );
    for method in ["PUT", "PATCH", "HEAD"] {
        let res = with_accept(method, "application/json");
        assert_eq!(res.status, 405, "{method} refused");
        assert_eq!(
            res.headers.get("content-length").map(String::as_str),
            Some("0"),
            "{method} refusal advertises an empty body"
        );
    }
}

#[test]
fn route_helpers_cover_security_and_status_paths() {
    let (port, handler) = spawn_server(HttpConfig::default());
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    // write_simple error writer via malformed request line. With no message the
    // advertised Content-Length must be zero (never a body-less truncation).
    let bad = request(port, "BOGUS\r\n\r\n");
    assert_eq!(bad.status, 400);
    assert_eq!(
        bad.headers.get("content-length").map(String::as_str),
        Some("0")
    );
    assert!(
        bad.body.is_empty(),
        "error bodies must match Content-Length"
    );
    // Constant-time bearer comparison: the helper is promoted to
    // clio_config::auth_tokens, whose own tests cover the comparison itself.
    // status_text table (every arm, including the unknown-code fallback).
    for (code, text) in [
        (200, "OK"),
        (202, "Accepted"),
        (400, "Bad Request"),
        (401, "Unauthorized"),
        (403, "Forbidden"),
        (404, "Not Found"),
        (405, "Method Not Allowed"),
        (406, "Not Acceptable"),
        (408, "Request Timeout"),
        (413, "Payload Too Large"),
        (415, "Unsupported Media Type"),
        (503, "Service Unavailable"),
        (599, "Error"),
    ] {
        assert_eq!(status_text(code), text);
    }
    // accepts_json: wildcard and SSE-only accepts pass; other media types fail.
    assert!(accepts_json("*/*"));
    assert!(accepts_json("text/event-stream"));
    assert!(!accepts_json("text/html"));
    // Origin policy table.
    assert!(origin_host_allowed("http://localhost:8080", &[]));
    assert!(origin_host_allowed("https://127.0.0.1", &[]));
    assert!(origin_host_allowed(
        "http://custom-host.example.com",
        &["custom-host.example.com".into()]
    ));
    assert!(!origin_host_allowed("http://evil.example.com", &[]));
    // handler + state accessors.
    let _ = handler.protocol_revision();
    assert_eq!(addr.port(), port);
}

#[test]
fn serve_with_listener_accepts_real_connections() {
    let state = crate::runtime::McpState::open_with_effective(
        &crate::runtime::McpOpenOptions::default(),
        &serde_json::json!({}),
    )
    .expect("state");
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let cfg = HttpConfig {
        insecure_no_auth: true,
        ..HttpConfig::default()
    };
    std::thread::spawn(move || {
        let _ = serve_with_listener(state, cfg, listener);
    });
    let res = post_served(port, "server/discover", "");
    assert_eq!(res.status, 200);
    let body: Value = serde_json::from_str(&res.body).expect("json");
    assert_eq!(
        body["result"]["supportedVersions"],
        json!([crate::MCP_PROTOCOL_REVISION])
    );
}

/// The recorded resource bounds are pinned: raising any of them silently is a
/// defect, because each one is the difference between a bounded and an
/// unbounded transport.
#[test]
fn resource_bounds_are_pinned_to_the_recorded_values() {
    assert_eq!(
        MAX_BODY,
        10 * 1024 * 1024,
        "10 MiB body cap must not change"
    );
    assert_eq!(MAX_HEAD_BYTES, 64 * 1024, "64 KiB head cap");
    assert_eq!(DEFAULT_DEADLINE_SECS, 30, "30 s total request deadline");
    assert_eq!(MAX_CONNECTIONS, 32, "32 concurrent-connection cap");
}
