// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the HTTP framing and the JSON POST client against a raw
//! one-shot TCP responder: request-line/header/body parsing, successful decode,
//! and every error path (refused connect, malformed body, wrong shape,
//! non-200 status), plus unit coverage for the read-bound error variants.
//!
//! ## Owns
//! - Unit coverage for `http` (`read_request` and `post_json`)
//! - The read-bound error variants (`ReadRequestError`) reachable over a socket
//!
//! ## Does not own
//! - Real-server bound enforcement and the payload measurement
//!   (`server_conn_tests`)
//! - Route semantics and apply behavior (`server_tests`)
//! - Client push/pull loops (`client_tests`)
//!
//! ## Boundary
//!
//! Tests here break when HTTP framing or its read-bound error variants
//! change; they never change it.
//!
//! Keep this module focused on HTTP framing and POST-client tests.

use super::{content_length, header_value, parse_route, post_json, read_request};
use crate::http::{BODY_CAP_BYTES, HEAD_CAP_BYTES, ReadRequestError};
use crate::protocol::{PullResponse, PushAck};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::{Duration, Instant};

/// A deadline far in the future for tests that do not exercise the deadline.
fn far_deadline() -> Instant {
    Instant::now() + Duration::from_secs(30)
}

/// One-shot TCP responder: accepts one connection, waits for the client to
/// finish sending, then writes `response` verbatim and closes.
fn respond_once(response: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr").to_string();
    thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            // Drain the request before closing; a close with unread bytes sends
            // RST and can discard the response the client already buffered.
            let _ = stream.set_read_timeout(Some(Duration::from_millis(50)));
            let mut buf = [0u8; 8192];
            loop {
                match stream.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
            }
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
            let _ = stream.shutdown(std::net::Shutdown::Write);
            thread::sleep(Duration::from_millis(5));
        }
    });
    addr
}

fn body() -> serde_json::Value {
    serde_json::json!({ "bank": "b1" })
}

#[test]
fn io_error_mappers_produce_internal_errors() {
    let io = || std::io::Error::other("boom");
    for (msg, err) in [
        ("read failed", super::io_read(io())),
        ("connect failed", super::io_connect(io())),
        ("timeout setup failed", super::io_timeout(io())),
        ("send failed", super::io_send(io())),
        ("recv failed", super::io_recv(io())),
    ] {
        assert!(err.message.contains(msg), "unexpected: {}", err.message);
        assert_eq!(err.code, clio_types::ErrorCode::Internal);
    }
    let bad = || serde_json::from_str::<serde_json::Value>("{").expect_err("bad json");
    assert!(
        super::io_bad_response(bad())
            .message
            .contains("bad response")
    );
    assert!(
        super::io_bad_shape(bad())
            .message
            .contains("bad response shape")
    );
}

#[test]
fn post_json_decodes_a_valid_response_for_both_shapes() {
    let addr = respond_once(
        "HTTP/1.1 200 OK\r\nContent-Length: 46\r\n\r\n\
         {\"cursor\":\"c1\",\"accepted\":[],\"rejected\":[]}",
    );
    let ack: PushAck = post_json(&addr, "/sync/push", None, &body()).expect("push ack");
    assert_eq!(ack.cursor, "c1");

    let addr = respond_once(
        "HTTP/1.1 200 OK\r\nContent-Length: 49\r\n\r\n\
         {\"mutations\":[],\"next_cursor\":\"n1\",\"has_more\":false}",
    );
    let page: PullResponse = post_json(&addr, "/sync/pull", Some("tok"), &body()).expect("page");
    assert_eq!(page.next_cursor, "n1");
    assert!(!page.has_more);
}

#[test]
fn post_json_refuses_a_dead_endpoint_for_both_shapes() {
    let cmd = post_json::<PushAck>("127.0.0.1:1", "/sync/push", None, &body());
    assert!(cmd.is_err());
    let page = post_json::<PullResponse>("127.0.0.1:1", "/sync/pull", None, &body());
    assert!(page.is_err());
}

#[test]
fn post_json_rejects_a_malformed_response_body() {
    let addr = respond_once("HTTP/1.1 200 OK\r\n\r\nnot-json{");
    assert!(post_json::<PushAck>(&addr, "/sync/push", None, &body()).is_err());
    let addr = respond_once("HTTP/1.1 200 OK\r\n\r\nnot-json{");
    assert!(post_json::<PullResponse>(&addr, "/sync/pull", None, &body()).is_err());
}

#[test]
fn post_json_rejects_a_wrong_response_shape() {
    let addr = respond_once("HTTP/1.1 200 OK\r\n\r\n{\"ok\":true}");
    assert!(post_json::<PushAck>(&addr, "/sync/push", None, &body()).is_err());
    let addr = respond_once("HTTP/1.1 200 OK\r\n\r\n{\"ok\":true}");
    assert!(post_json::<PullResponse>(&addr, "/sync/pull", None, &body()).is_err());
}

#[test]
fn post_json_rejects_a_non_200_status() {
    let addr = respond_once("HTTP/1.1 403 Forbidden\r\n\r\n{\"message\":\"no\"}");
    let err = post_json::<PushAck>(&addr, "/sync/push", None, &body()).expect_err("403");
    assert!(err.message.contains("403"));
}

#[test]
fn read_request_parses_route_auth_and_body() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr").to_string();
    thread::spawn(move || {
        let mut client = TcpStream::connect(addr).expect("connect");
        client
            .write_all(b"POST /sync/push HTTP/1.1\r\nAuthorization: Bearer t1\r\nContent-Length: 2\r\n\r\n{}")
            .expect("write");
        client.flush().expect("flush");
    });
    let (mut stream, _) = listener.accept().expect("accept");
    let request = read_request(&mut stream, far_deadline()).expect("read request");
    assert!(matches!(request.route, Some(super::Route::Push)));
    assert_eq!(request.authorization.as_deref(), Some("Bearer t1"));
    assert_eq!(request.body, "{}");
}

#[test]
fn read_request_assembles_a_body_split_across_reads() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr").to_string();
    thread::spawn(move || {
        let mut client = TcpStream::connect(addr).expect("connect");
        client
            .write_all(b"POST /sync/pull HTTP/1.1\r\nContent-Length: 5\r\n\r\n")
            .expect("head");
        client.flush().expect("flush");
        thread::sleep(Duration::from_millis(40));
        client.write_all(b"hello").expect("body");
        client.flush().expect("flush");
    });
    let (mut stream, _) = listener.accept().expect("accept");
    let request = read_request(&mut stream, far_deadline()).expect("read request");
    assert!(matches!(request.route, Some(super::Route::Pull)));
    assert_eq!(request.body, "hello");
}

#[test]
fn read_request_refuses_a_header_block_over_the_head_cap() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr").to_string();
    thread::spawn(move || {
        let mut client = TcpStream::connect(addr).expect("connect");
        // Over the cap, no terminator: the block alone must be refused.
        let head = format!(
            "POST /sync/push HTTP/1.1\r\nX-Filler: {}\r\n",
            "f".repeat(HEAD_CAP_BYTES + 8192)
        );
        let _ = client.write_all(head.as_bytes());
        let _ = client.flush();
    });
    let (mut stream, _) = listener.accept().expect("accept");
    assert!(
        matches!(
            read_request(&mut stream, far_deadline()),
            Err(ReadRequestError::OversizeHead)
        ),
        "a header block over the head cap must be refused in the loop"
    );
}

#[test]
fn read_request_refuses_a_declared_body_over_the_body_cap() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr").to_string();
    thread::spawn(move || {
        let mut client = TcpStream::connect(addr).expect("connect");
        // Declared over-cap length and no body bytes: refused before reading.
        let head = format!(
            "POST /sync/pull HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
            BODY_CAP_BYTES + 1
        );
        let _ = client.write_all(head.as_bytes());
        let _ = client.flush();
        // Keep the socket open briefly: the refusal must not wait for a body.
        thread::sleep(Duration::from_millis(100));
    });
    let (mut stream, _) = listener.accept().expect("accept");
    assert!(
        matches!(
            read_request(&mut stream, far_deadline()),
            Err(ReadRequestError::OversizeBody)
        ),
        "a declared body over the cap must be refused before body reads"
    );
}

#[test]
fn read_request_refuses_an_already_expired_deadline() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr").to_string();
    // The client sends nothing and closes: a past deadline must be reported
    // without any read completing a request.
    thread::spawn(move || {
        let client = TcpStream::connect(addr).expect("connect");
        thread::sleep(Duration::from_millis(50));
        drop(client);
    });
    let (mut stream, _) = listener.accept().expect("accept");
    let expired = Instant::now()
        .checked_sub(Duration::from_secs(1))
        .expect("a past instant");
    assert!(
        matches!(
            read_request(&mut stream, expired),
            Err(ReadRequestError::Deadline)
        ),
        "an expired deadline must surface as the Deadline variant"
    );
}

#[test]
fn framing_helpers_parse_lines_and_headers() {
    assert!(matches!(
        parse_route("GET /sync/status HTTP/1.1"),
        Some(super::Route::Status)
    ));
    assert!(matches!(
        parse_route("POST /sync/pull HTTP/1.1"),
        Some(super::Route::Pull)
    ));
    assert!(parse_route("GET /nope HTTP/1.1").is_none());
    assert!(parse_route("garbage").is_none());
    let head = "POST /sync/push HTTP/1.1\r\nContent-Length: 12\r\nAuthorization: Bearer z\r\n\r\n";
    assert_eq!(content_length(head), Some(12));
    assert_eq!(
        header_value(head, "authorization").as_deref(),
        Some("Bearer z")
    );
    assert_eq!(header_value(head, "missing"), None);
    assert_eq!(content_length("POST /x HTTP/1.1\r\n\r\n"), None);
}
