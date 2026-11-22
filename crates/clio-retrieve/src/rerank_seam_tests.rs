// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests of the fake rerank service and the
//! HTTP request decoder the batch tests and the end-to-end rerank tests drive.
//!
//! ## Owns
//! - The interrupted-read retry test and the truncated-capture failure test
//! - The end-to-end check that the fake service answers and returns its capture
//!
//! ## Does not own
//! - The fake service and decoder themselves (`rerank_seam`)
//! - Any test of the adapter's batching or parsing (`rerank_batch_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the seam's own guards.

use super::{batch_server, json_body, read_request_body, texts};
use std::io::{Cursor, Read, Write};

/// A reader that reports one interrupted read, then serves `bytes` normally.
struct InterruptOnce {
    bytes: Cursor<Vec<u8>>,
    interrupted: bool,
}

impl Read for InterruptOnce {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if !self.interrupted {
            self.interrupted = true;
            return Err(std::io::Error::from(std::io::ErrorKind::Interrupted));
        }
        self.bytes.read(buf)
    }
}

fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<&str>() {
        (*text).to_owned()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        String::new()
    }
}

#[test]
fn an_interrupted_read_is_retried_and_the_full_request_is_captured() {
    let body = r#"{"texts":["a"]}"#;
    let request = format!(
        "POST /rerank HTTP/1.1\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
        body.len()
    );
    let mut reader = InterruptOnce {
        bytes: Cursor::new(request.into_bytes()),
        interrupted: false,
    };

    let captured = read_request_body(&mut reader);

    assert!(
        captured.contains("\r\n\r\n"),
        "captured request lacks a header terminator: {captured:?}"
    );
    assert_eq!(json_body(&captured)["texts"], serde_json::json!(["a"]));
}

#[test]
fn a_read_that_ends_before_the_header_block_reports_the_byte_count_and_kind() {
    let partial = b"POST /rerank HTTP/1.1\r\ncontent-length: 2\r\n";
    let mut reader = &partial[..];
    let payload = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        read_request_body(&mut reader)
    }))
    .expect_err("a read that ends before the header block must panic");

    let message = panic_text(&*payload);
    assert!(
        message.contains(&format!(
            "read ended after {} bytes before the header block",
            partial.len()
        )),
        "{message}"
    );
    assert!(message.contains("UnexpectedEof"), "{message}");
}

#[test]
fn batch_server_answers_and_returns_the_request_it_captured() {
    let response = "HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\n{}";
    let (url, handle) = batch_server(vec![response.to_owned()]);
    let body = r#"{"texts":[]}"#;
    let request = format!(
        "POST /rerank HTTP/1.1\r\ncontent-length: {}\r\n\r\n{body}",
        body.len()
    );
    let mut stream =
        std::net::TcpStream::connect(url.trim_start_matches("http://")).expect("connect");
    stream.write_all(request.as_bytes()).expect("write request");
    let mut reply = String::new();
    stream.read_to_string(&mut reply).expect("read response");

    assert_eq!(reply, response);
    let captured = handle.join().expect("server thread");
    assert_eq!(texts(&captured[0]), Vec::<String>::new());
}

#[test]
fn json_body_names_the_capture_when_the_header_terminator_is_missing() {
    let request = "GET /rerank HTTP/1.1\r\nhost: localhost\r\n";
    let payload = std::panic::catch_unwind(|| json_body(request))
        .expect_err("a request with no header terminator must panic");

    let message = panic_text(&*payload);
    assert!(
        message.contains(&format!("request is {} bytes", request.len())),
        "{message}"
    );
    assert!(message.contains("GET /rerank HTTP/1.1"), "{message}");
}
