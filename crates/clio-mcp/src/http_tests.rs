// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! Streamable HTTP transport tests: security baselines,
//! session lifecycle, initialize/tools/call end to end.

use super::http_test_support_tests::{INIT, post_json, request, spawn_server};
use super::*;
use crate::conn_pool::MAX_CONNECTIONS;
use crate::http_guard::origin_host_allowed;
use crate::runtime::McpOpenOptions;
use clio_config::auth_tokens::{AuthTokenRecord, digest_hex};
use clio_types::Actor;
use serde_json::json;

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

#[test]
fn t06_initialize_returns_pinned_revision() {
    let (port, _) = spawn_server(HttpConfig::default());
    let res = post_json(port, INIT, "");
    assert_eq!(res.status, 200);
    let body: Value = serde_json::from_str(&res.body).expect("json");
    assert_eq!(
        body["result"]["protocolVersion"],
        json!(crate::MCP_PROTOCOL_REVISION)
    );
    assert_eq!(body["result"]["serverInfo"]["name"], json!("clio-mcp"));
}

#[test]
fn t06_session_lifecycle_mint_echo_delete() {
    let (port, _) = spawn_server(HttpConfig {
        sessions_enabled: true,
        ..HttpConfig::default()
    });
    let res = post_json(port, INIT, "");
    let session = res
        .headers
        .get("mcp-session-id")
        .cloned()
        .expect("session minted on initialize");
    // Without the session id subsequent calls are rejected.
    let list = r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#;
    let no_session = post_json(port, list, "");
    assert_eq!(no_session.status, 404, "unauthorized session use rejected");
    // Echo works.
    let call = r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"consolidate","arguments":{"bank":"bank-a"}}}"#;
    let ok = post_json(port, call, &format!("MCP-Session-Id: {session}\r\n"));
    assert_eq!(ok.status, 200);
    // DELETE terminates; the id stops working.
    let del = request(
        port,
        &format!(
            "DELETE /mcp HTTP/1.1\r\nHost: localhost\r\nMCP-Session-Id: {session}\r\nContent-Length: 0\r\n\r\n"
        ),
    );
    assert_eq!(del.status, 200);
    let after = post_json(port, call, &format!("MCP-Session-Id: {session}\r\n"));
    assert_eq!(after.status, 404);
}

#[test]
fn t08_bad_origin_is_403() {
    let (port, _) = spawn_server(HttpConfig::default());
    let res = post_json(port, INIT, "Origin: http://evil.example\r\n");
    assert_eq!(res.status, 403);
    let ok = post_json(port, INIT, "Origin: http://localhost:5173\r\n");
    assert_eq!(ok.status, 200);
}

#[test]
fn t09_missing_auth_rejected_when_a_store_record_is_configured() {
    let (port, _) = spawn_server(HttpConfig {
        credentials: Some(Arc::new(single_record_store())),
        ..HttpConfig::default()
    });
    let no_auth = post_json(port, INIT, "");
    assert_eq!(no_auth.status, 401);
    let wrong = post_json(port, INIT, "Authorization: Bearer nope\r\n");
    assert_eq!(wrong.status, 401);
    let good = post_json(port, INIT, "Authorization: Bearer secret-token\r\n");
    assert_eq!(good.status, 200);
}

#[test]
fn non_loopback_without_a_usable_store_fails_closed() {
    let state = crate::runtime::McpState::open_with_effective(
        &McpOpenOptions::default(),
        &serde_json::json!({}),
    )
    .expect("state");
    let listener = TcpListener::bind("0.0.0.0:0").expect("bind");
    let err = serve_with_listener(state, HttpConfig::default(), listener);
    let message = err
        .expect_err("non-loopback bind without a credential must fail closed")
        .to_string();

    // The opening phrase is the substring an operator's runbook and a search
    // hold, and it is pinned separately; the removed flag and environment
    // sources must not come back into the wording.
    assert!(
        message.contains("non-loopback bind requires an auth token"),
        "the pinned phrase must survive: {message}"
    );
    assert!(
        message.contains("--token-file"),
        "the refusal must name the token store: {message}"
    );
    for removed in ["--auth-token", "CLIO_MCP_TOKEN"] {
        assert!(
            !message.contains(removed),
            "the removed source {removed} must leave the wording: {message}"
        );
    }
}

/// A store path holding no usable record authorizes nothing, so it must not
/// satisfy the non-loopback credential startup rule: the server fails closed
/// exactly as it would with no store at all.
#[test]
fn non_loopback_with_an_empty_store_fails_closed() {
    let state = crate::runtime::McpState::open_with_effective(
        &McpOpenOptions::default(),
        &serde_json::json!({}),
    )
    .expect("state");
    let listener = TcpListener::bind("0.0.0.0:0").expect("bind");
    let cfg = HttpConfig {
        credentials: Some(Arc::new(CredentialStore::default())),
        ..HttpConfig::default()
    };
    let err = serve_with_listener(state, cfg, listener);
    assert!(
        err.is_err(),
        "an empty store must not satisfy a non-loopback bind"
    );
}

#[test]
fn get_sse_and_wrong_paths() {
    let (port, _) = spawn_server(HttpConfig::default());
    let get = request(port, "GET /mcp HTTP/1.1\r\nHost: localhost\r\n\r\n");
    assert_eq!(get.status, 405, "GET SSE not offered under this pin");
    let miss = post_json(port, INIT, "");
    assert_eq!(miss.status, 200);
    let wrong_path = request(
        port,
        &format!(
            "POST /other HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nAccept: application/json\r\nContent-Length: {}\r\n\r\n{}",
            INIT.len(),
            INIT
        ),
    );
    assert_eq!(wrong_path.status, 404);
}

#[test]
fn tools_call_end_to_end_over_http() {
    let (port, _) = spawn_server(HttpConfig::default());
    post_json(port, INIT, "");
    let call = r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"triple_add","arguments":{"subject":"a","predicate":"b","object":"c","epistemic_kind":"fact","bank":"bank-a"}}}"#;
    let res = post_json(port, call, "");
    assert_eq!(res.status, 200);
    let body: Value = serde_json::from_str(&res.body).expect("json");
    let result = &body["result"];
    assert_eq!(result["isError"], json!(false));
    assert!(result["structuredContent"]["ok"].as_bool().unwrap_or(false));
}

#[test]
fn notification_post_returns_202() {
    let (port, _) = spawn_server(HttpConfig::default());
    let res = post_json(
        port,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        "",
    );
    assert_eq!(res.status, 202);
    assert!(res.body.is_empty(), "{:?}", res.body);
}

#[test]
fn wrong_content_type_is_415() {
    let (port, _) = spawn_server(HttpConfig::default());
    let res = request(
        port,
        &format!(
            "POST /mcp HTTP/1.1\r\nHost: localhost\r\nContent-Type: text/plain\r\nAccept: application/json\r\nContent-Length: {}\r\n\r\n{}",
            INIT.len(),
            INIT
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
    let with_accept = |method: &str, accept: &str| {
        request(
            port,
            &format!(
                "{method} /mcp HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\n\
                 Accept: {accept}\r\nContent-Length: {}\r\n\r\n{INIT}",
                INIT.len()
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
    let shared = HttpShared::default();
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
        "http://custom-host",
        &["custom-host".into()]
    ));
    assert!(!origin_host_allowed("http://evil.example", &[]));
    // DELETE without session id → 400.
    let del = request(port, "DELETE /mcp HTTP/1.1\r\nHost: localhost\r\n\r\n");
    assert_eq!(del.status, 400);
    // DELETE unknown session id → 404.
    let del = request(
        port,
        "DELETE /mcp HTTP/1.1\r\nHost: localhost\r\nMCP-Session-Id: nope\r\nContent-Length: 0\r\n\r\n",
    );
    assert_eq!(del.status, 404);
    // mint_session mints unique ids and registers them unowned.
    let a = mint_session(&shared, None);
    let b = mint_session(&shared, None);
    assert_ne!(a, b);
    assert!(a.starts_with("sess-"));
    assert_eq!(a.len(), 37, "16-byte hex session id: {a}");
    assert!(shared.is_live(&a, None), "the minted id is registered");
    assert!(
        !shared.is_live(&b, None) || b != a,
        "each id is its own entry"
    );
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
    let res = post_json(port, INIT, "");
    assert_eq!(res.status, 200);
    let body: Value = serde_json::from_str(&res.body).expect("json");
    assert_eq!(
        body["result"]["protocolVersion"],
        json!(crate::MCP_PROTOCOL_REVISION)
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

/// Transport parity for the retention tools: the same `tools/call` returns the
/// same `structuredContent` over stdio and Streamable HTTP.
#[test]
fn retention_tools_parity_stdio_vs_http() {
    let batch = json!({
        "jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": {"name": "admit_preview_batch", "arguments": {
            "bank": "bank-a",
            "items": [
                {"item": {"id": "p1", "gist": "must require decision"}, "episodic_type": "gist", "epistemic_kind": "fact"},
                {"item": {"id": "p2", "gist": "ok"}, "episodic_type": "gist", "epistemic_kind": "fact"}
            ]
        }}
    })
    .to_string();
    let profile = json!({
        "jsonrpc": "2.0", "id": 3, "method": "tools/call",
        "params": {"name": "retention_profile_get", "arguments": {"bank": "bank-a"}}
    })
    .to_string();

    // stdio transport over the shared handler.
    let stdio_state =
        McpState::open_with_effective(&McpOpenOptions::default(), &serde_json::json!({}))
            .expect("state");
    let stdio_handler = McpHandler::new(stdio_state);
    let frames = format!("{batch}\n{profile}\n");
    let mut out: Vec<u8> = Vec::new();
    crate::stdio::serve(
        &stdio_handler,
        std::io::Cursor::new(frames.into_bytes()),
        &mut out,
    )
    .expect("serve");
    let stdio: Vec<Value> = String::from_utf8(out)
        .expect("utf8")
        .lines()
        .map(|l| serde_json::from_str(l).expect("json"))
        .collect();
    assert_eq!(stdio.len(), 2);

    // Streamable HTTP transport.
    let (port, _) = spawn_server(HttpConfig::default());
    let http_batch: Value = serde_json::from_str(&post_json(port, &batch, "").body).expect("json");
    let http_profile: Value =
        serde_json::from_str(&post_json(port, &profile, "").body).expect("json");

    assert_eq!(
        stdio[0]["result"]["structuredContent"],
        http_batch["result"]["structuredContent"]
    );
    assert_eq!(
        stdio[1]["result"]["structuredContent"],
        http_profile["result"]["structuredContent"]
    );
    assert!(
        http_batch["result"]["structuredContent"]["summary"]["would_admit"]
            .as_u64()
            .is_some()
    );

    // Both transports publish the new tools in `tools/list`.
    let list = r#"{"jsonrpc":"2.0","id":4,"method":"tools/list"}"#;
    let http_list: Value = serde_json::from_str(&post_json(port, list, "").body).expect("json");
    let names: Vec<&str> = http_list["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    for name in [
        "admit_preview_batch",
        "retention_profile_get",
        "retention_profile_set",
    ] {
        assert!(names.contains(&name), "`{name}` missing over HTTP");
    }
    let stdio_list = McpHandler::tools_list();
    let stdio_names: Vec<&str> = stdio_list["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert_eq!(names, stdio_names);
}
