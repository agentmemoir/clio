// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the Streamable HTTP transport conformance
//! suite: the write-set parity matrix over HTTP, the Origin gate, discovery,
//! the removed handshake, the revision and header gates, the auth gate, and the
//! verb and method refusals.
//!
//! ## Owns
//! - `spawn_http`, `post_admitted`, and `http_request`, the raw HTTP client
//!   harness that carries the conditional headers the admission requires.
//! - The HTTP write-set parity and Origin test, the discovery and removed-method
//!   test, the revision and header refusal test, the auth reject test, and the
//!   GET and unknown-method test.
//!
//! ## Does not own
//! - The shared write-set fixture and builders (`main`).
//! - The stdio transport suite (`stdio`).
//!
//! ## Boundary
//!
//! Changes to HTTP status gates, the conditional headers, or the raw request
//! harness should normally require changes here. Changes to the write-set
//! fixture should not require changes here.
//!
//! Keep this module focused on HTTP transport conformance.

use clio_mcp::MCP_PROTOCOL_REVISION;
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;

use crate::{mask_volatile, request, rpc, state, tool_call, tools_list, write_set_fixture};

/// Spawn the real Streamable HTTP server on an ephemeral loopback port.
fn spawn_http(cfg: clio_mcp::http::HttpConfig) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let st = state();
    std::thread::spawn(move || {
        let mut cfg = cfg;
        cfg.insecure_no_auth = true;
        let _ = clio_mcp::http::serve_with_listener(st, cfg, listener);
    });
    addr
}

/// Send one raw request and read back the status and the JSON body.
fn http_request(addr: &SocketAddr, raw: &str) -> (u16, String, Value) {
    let mut stream = TcpStream::connect(addr).expect("connect");
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
    stream.write_all(raw.as_bytes()).expect("write");
    let mut buf = Vec::new();
    let _ = stream.read_to_end(&mut buf);
    let pos = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .unwrap_or(buf.len());
    let head = String::from_utf8_lossy(&buf[..pos]).into_owned();
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let body = String::from_utf8_lossy(&buf[pos + 4..]).into_owned();
    (
        status,
        body.clone(),
        serde_json::from_str::<Value>(&body).unwrap_or(Value::Null),
    )
}

/// The conditional headers one admitted request carries.
fn admitted_headers(method: &str, name: Option<&str>) -> String {
    let mut headers =
        format!("Mcp-Protocol-Version: {MCP_PROTOCOL_REVISION}\r\nMcp-Method: {method}\r\n");
    if let Some(name) = name {
        headers = format!("{headers}Mcp-Name: {name}\r\n");
    }
    headers
}

/// POST `body` as `method`, carrying the conditional headers and `extra`.
fn post_admitted(
    addr: &SocketAddr,
    method: &str,
    name: Option<&str>,
    body: &str,
    extra: &str,
) -> (u16, Value) {
    let req = format!(
        "POST /mcp HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nContent-Length: {}\r\n{}{extra}\r\n{body}",
        body.len(),
        admitted_headers(method, name)
    );
    let (status, _, parsed) = http_request(addr, &req);
    (status, parsed)
}

#[test]
fn http_write_set_matrix_with_parity_and_security() {
    let addr = spawn_http(clio_mcp::http::HttpConfig::default());
    let fixtures = write_set_fixture();
    let direct_state = state();
    let direct: Vec<Value> = fixtures
        .iter()
        .map(|(name, args)| {
            clio_mcp::write_tools::dispatch_in_scope(
                &direct_state,
                name,
                args,
                clio_mcp::ToolScope::Advertised,
            )
        })
        .collect();
    // Invalid Origin is rejected before any protocol work.
    let (status, _) = post_admitted(
        &addr,
        "server/discover",
        None,
        &request(1, "server/discover"),
        "Origin: http://evil.example.com\r\n",
    );
    assert_eq!(status, 403, "bad Origin rejected");
    for (i, (name, args)) in fixtures.iter().enumerate() {
        let body = tool_call(100 + i as u64, name, args);
        let (status, resp) = post_admitted(&addr, "tools/call", Some(name), &body, "");
        assert_eq!(status, 200, "stateless transport accepts `{name}`");
        let structured = resp["result"]["structuredContent"].clone();
        assert_eq!(
            mask_volatile(structured),
            mask_volatile(direct[i].clone()),
            "stdio/HTTP parity mismatch on `{name}`"
        );
        let is_error = resp["result"]["isError"].as_bool().expect("isError");
        let ok = direct[i]["ok"].as_bool().expect("ok");
        assert_eq!(is_error, !ok, "isError mirrors ok for `{name}`");
    }
}

#[test]
fn t06_http_discovery_and_removed_handshake() {
    let addr = spawn_http(clio_mcp::http::HttpConfig::default());
    let (status, discovered) = post_admitted(
        &addr,
        "server/discover",
        None,
        &request(1, "server/discover"),
        "",
    );
    assert_eq!(status, 200);
    let result = &discovered["result"];
    assert_eq!(result["resultType"], json!("complete"));
    assert_eq!(result["supportedVersions"], json!([MCP_PROTOCOL_REVISION]));
    assert_eq!(
        result["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
        json!("clio-mcp")
    );
    assert!(result.get("serverInfo").is_none(), "no top-level identity");

    // The retired handshake answers as an unknown method, at the not-found
    // status, and never mints a session.
    for method in ["initialize", "ping"] {
        let (status, refused) = post_admitted(&addr, method, None, &request(2, method), "");
        assert_eq!(status, 404, "`{method}` is not served");
        assert_eq!(refused["error"]["code"], json!(-32601), "`{method}`");
        assert!(refused.get("result").is_none(), "`{method}` served");
    }

    // A notification is accepted with no body and no content, and it carries
    // the same required metadata every other message carries.
    let notification = json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized",
        "params": {"_meta": crate::required_meta()}
    })
    .to_string();
    let req = format!(
        "POST /mcp HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nContent-Length: {}\r\n{}\r\n{notification}",
        notification.len(),
        admitted_headers("notifications/initialized", None)
    );
    let (status, body, _) = http_request(&addr, &req);
    assert_eq!(status, 202, "a notification is accepted");
    assert!(body.is_empty(), "a notification answers no body: {body}");

    // The same notification without its metadata is refused as invalid params.
    let bare = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
    let req = format!(
        "POST /mcp HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nContent-Length: {}\r\n{}\r\n{bare}",
        bare.len(),
        admitted_headers("notifications/initialized", None)
    );
    let (status, _, refused) = http_request(&addr, &req);
    assert_eq!(status, 400, "the metadata is required on a notification");
    assert_eq!(refused["error"]["code"], json!(-32602), "{refused}");

    // Both transports advertise the selected profile's tools over HTTP, and the
    // wider catalogue those tools belong to stays bound.
    let (status, listed) = post_admitted(&addr, "tools/list", None, &tools_list(2), "");
    assert_eq!(status, 200);
    let names: Vec<&str> = listed["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    let bound = clio_mcp::bound_tools();
    let expected: Vec<&str> = clio_types::ToolPreset::All
        .tools()
        .into_iter()
        .filter(|name| bound.contains(name))
        .collect();
    assert_eq!(
        names.len(),
        expected.len(),
        "the HTTP listing follows the selected profile"
    );
    for name in expected {
        assert!(
            names.contains(&name),
            "`{name}` missing from HTTP tools/list"
        );
    }
}

/// Every request must name the revision twice and agree with itself: the
/// header is required, must equal the request metadata, and must be the served
/// revision. Each refusal is a distinct code, so a client can tell them apart.
#[test]
fn t07_http_requires_the_version_header_and_the_served_revision() {
    let addr = spawn_http(clio_mcp::http::HttpConfig::default());
    let body = request(1, "server/discover");

    // No `MCP-Protocol-Version` header at all.
    let missing = format!(
        "POST /mcp HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nAccept: application/json\r\nContent-Length: {}\r\nMcp-Method: server/discover\r\n\r\n{body}",
        body.len()
    );
    let (status, _, refused) = http_request(&addr, &missing);
    assert_eq!(status, 400, "the version header is required");
    assert_eq!(refused["error"]["code"], json!(-32020), "{refused}");

    // A header that disagrees with the request metadata.
    let mismatch = format!(
        "POST /mcp HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nAccept: application/json\r\nContent-Length: {}\r\nMcp-Protocol-Version: 1900-01-01\r\nMcp-Method: server/discover\r\n\r\n{body}",
        body.len()
    );
    let (status, _, refused) = http_request(&addr, &mismatch);
    assert_eq!(status, 400, "header and metadata must agree");
    assert_eq!(refused["error"]["code"], json!(-32020), "{refused}");

    // A client still opening on the predecessor revision is refused, and the
    // refusal names both revisions.
    let retired = rpc(
        1,
        "server/discover",
        json!({
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": "2025-11-25",
                "io.modelcontextprotocol/clientCapabilities": {}
            }
        }),
    );
    let refused_revision = format!(
        "POST /mcp HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nAccept: application/json\r\nContent-Length: {}\r\nMcp-Protocol-Version: 2025-11-25\r\nMcp-Method: server/discover\r\n\r\n{retired}",
        retired.len()
    );
    let (status, _, refused) = http_request(&addr, &refused_revision);
    assert_eq!(status, 400);
    assert_eq!(refused["error"]["code"], json!(-32022), "{refused}");
    assert_eq!(
        refused["error"]["data"]["supported"],
        json!([MCP_PROTOCOL_REVISION])
    );
    assert_eq!(refused["error"]["data"]["requested"], json!("2025-11-25"));

    // A request with no metadata at all is refused as invalid params.
    let bare = r#"{"jsonrpc":"2.0","id":1,"method":"server/discover"}"#;
    let (status, refused) = post_admitted(&addr, "server/discover", None, bare, "");
    assert_eq!(status, 400);
    assert_eq!(refused["error"]["code"], json!(-32602), "{refused}");
}

#[test]
fn t09_non_dev_missing_auth_rejected() {
    // A store record is the only credential a server accepts; the scratch file is removed after the load.
    let (record, secret) =
        clio_config::auth_tokens::new_auth_token("conf", "harness", &["conf-bank".to_owned()])
            .expect("record");
    let path = std::env::temp_dir().join(format!("cf-t09-{}.json", std::process::id()));
    clio_config::auth_tokens::save_auth_tokens(&path, &[record]).expect("save");
    #[cfg(unix)]
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).expect("tighten");
    let store = clio_mcp::http_auth::CredentialStore::load(&path).expect("load");
    let _ = std::fs::remove_file(&path);
    let addr = spawn_http(clio_mcp::http::HttpConfig {
        credentials: Some(Arc::new(store)),
        ..clio_mcp::http::HttpConfig::default()
    });
    let body = request(1, "server/discover");
    let (status, _) = post_admitted(&addr, "server/discover", None, &body, "");
    assert_eq!(status, 401, "missing auth rejected");
    let (status, _) = post_admitted(
        &addr,
        "server/discover",
        None,
        &body,
        "Authorization: Bearer wrong\r\n",
    );
    assert_eq!(status, 401, "wrong token rejected");
    let good = format!("Authorization: Bearer {secret}\r\n");
    let (status, discovered) = post_admitted(&addr, "server/discover", None, &body, &good);
    assert_eq!(status, 200);
    assert_eq!(
        discovered["result"]["supportedVersions"],
        json!([MCP_PROTOCOL_REVISION])
    );
}

#[test]
fn http_get_sse_refusal_and_unknown_method() {
    let addr = spawn_http(clio_mcp::http::HttpConfig::default());
    let (status, body, _) = http_request(&addr, "GET /mcp HTTP/1.1\r\nHost: localhost\r\n\r\n");
    assert_eq!(status, 405, "no server-initiated SSE stream is offered");
    assert!(body.is_empty(), "the verb refusal carries no body: {body}");
    let unknown = rpc(
        9,
        "resources/list",
        json!({"_meta": crate::required_meta()}),
    );
    let (status, refused) = post_admitted(&addr, "resources/list", None, &unknown, "");
    assert_eq!(status, 404, "an unrouted method is not served");
    assert_eq!(refused["error"]["code"], json!(-32601));
}
