// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the two per-request resource bounds that only
//! a real server can prove, both against the real `sync_serve` entry point over
//! a real socket: a connection that sends nothing at all is cut at the total
//! request deadline, and a header block that fits inside the head cap is served
//! even when the read carrying its terminator crosses the cap.
//!
//! ## Owns
//! - The idle-connection deadline cut, end to end against the real server
//! - The header block sitting exactly on the head cap, and the same block
//!   arriving in two reads so its terminating read crosses the cap
//!
//! ## Does not own
//! - The accept loop, pool hand-off, connection cap, and the other bound
//!   refusals (`server_conn_tests`)
//! - The read-loop unit variants of the same bounds (`http_tests`)
//! - Pool accounting unit coverage (`conn_pool_tests`)
//!
//! ## Boundary
//!
//! Changes to the head cap, the total request deadline, or how either bound is
//! reached should normally require changes here. Changes to connection
//! acceptance, slot accounting, or the refusal statuses should not require
//! changes here (`server_conn`, `conn_pool`).
//!
//! Keep this module focused on the two bounds a real server has to prove.

use super::server_tests::{raw_exchange, serve, status_of};
use crate::http::HEAD_CAP_BYTES;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::thread;
use std::time::{Duration, Instant};

/// One `GET /sync/status` request whose whole header block, terminator
/// included, is exactly `HEAD_CAP_BYTES` bytes. The filler absorbs the length of
/// everything else, so the total is exact and independent of the port the OS
/// assigns. `content_length`, when given, adds the declaration line before the
/// filler is sized, so the block still lands on the cap exactly.
fn head_on_the_cap(addr: &str, content_length: Option<usize>) -> String {
    let declared = content_length
        .map(|n| format!("Content-Length: {n}\r\n"))
        .unwrap_or_default();
    let bare = format!(
        "GET /sync/status HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer t1\r\n{declared}\
         X-Filler: \r\nConnection: close\r\n\r\n"
    );
    let filler = "f".repeat(HEAD_CAP_BYTES - bare.len());
    bare.replace("X-Filler: \r\n", &format!("X-Filler: {filler}\r\n"))
}

/// Send one request as two writes with a pause between them, so the read that
/// carries the header terminator also carries bytes past the cap.
///
/// `raw_exchange` writes the request in a single call and cannot express this:
/// it is the *landing* of the terminator-carrying read relative to the cap that
/// is under test, so the client has to choose where the split falls and let the
/// first half drain before the second half is written. The pause is orders of
/// magnitude above the loopback cost of draining 62 KB, so the first half is
/// always fully consumed first; without it the two halves could land in one
/// read and the split would prove nothing.
fn exchange_split(addr: &str, first: &str, second: &[u8]) -> (u16, String) {
    let mut stream = TcpStream::connect(addr).expect("connect");
    // Both directions are bounded, so a server that never answers fails this
    // test instead of blocking the test thread forever.
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .expect("client read timeout");
    stream
        .set_write_timeout(Some(Duration::from_secs(30)))
        .expect("client write timeout");
    stream.write_all(first.as_bytes()).expect("first half");
    stream.flush().expect("flush first half");
    thread::sleep(Duration::from_millis(200));
    stream.write_all(second).expect("second half");
    stream.flush().expect("flush second half");
    let mut raw = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => raw.extend_from_slice(&buf[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => break,
            // A client-side read timeout is a failure, not a bounded end: it
            // would otherwise read as an empty response and hide the real fault.
            Err(e) => panic!("read response failed: {e}"),
        }
    }
    let text = String::from_utf8_lossy(&raw).into_owned();
    (status_of(&text), text)
}

/// The head cap bounds the *completed* head, not the buffer as it stood before
/// a chunk was appended. Two shapes of the same 64 KiB block are sent and both
/// must be answered 200:
///
/// - the whole block in one write, with the terminator landing on the cap;
/// - the same block in two writes, so the read carrying the terminator also
///   carries the declared body past the cap. Judging the head before appending
///   that chunk refuses the second shape with 413 even though the block is
///   inside the cap, which is the defect this test pins.
#[test]
fn head_cap_serves_a_header_block_that_ends_on_the_cap() {
    let server = serve(&["b1"], Some("t1"), false, None, None).0;
    let addr = server.addr().to_owned();

    let head = head_on_the_cap(&addr, None);
    assert_eq!(
        head.len(),
        HEAD_CAP_BYTES,
        "the head must sit exactly on the cap"
    );
    let (status, text) = raw_exchange(&addr, head.as_bytes());
    assert_eq!(
        status, 200,
        "a header block inside the cap must be served: {text}"
    );

    // A well-formed push body, so the second request is framed like a real one
    // rather than a head with junk trailing it. The declaration is part of the
    // 64 KiB block, so the block still ends exactly on the cap.
    let body = r#"{"device_id":"dev-a","bank":"b1","mutations":[]}"#;
    let head = head_on_the_cap(&addr, Some(body.len()));
    assert_eq!(
        head.len(),
        HEAD_CAP_BYTES,
        "the head must sit exactly on the cap"
    );
    // 62000 is not a multiple of the 4096-byte read size, so the read that
    // carries the terminator cannot land exactly on the cap: it always crosses
    // it. The first half must therefore hold no terminator of its own.
    let (first, second) = head.split_at(62_000);
    assert!(
        !first.contains("\r\n\r\n"),
        "the first half must hold no terminator, or the split proves nothing"
    );
    let mut tail = second.as_bytes().to_vec();
    tail.extend_from_slice(body.as_bytes());
    let (status, text) = exchange_split(&addr, first, &tail);
    assert_eq!(
        status, 200,
        "a head inside the cap must be served when its terminator arrives in the \
         read that crosses the cap: {text}"
    );
    server.stop();
}

/// A connection that sends nothing at all is cut at the total request deadline,
/// the same point a slow-dripping client is cut. This is the one bound a unit
/// test cannot prove: the read-loop variant uses an already-expired deadline, so
/// nothing ever waits.
///
/// The reader runs on its own thread and the main thread only joins, so a
/// refusal cannot be lost. `join` cannot hang: the socket read timeout below
/// bounds the reader's loop, so the join returns at the latest when it fires.
#[test]
fn idle_connection_is_cut_at_the_total_deadline() {
    let server = serve(&["b1"], Some("t1"), false, None, Some(2)).0;
    let addr = server.addr().to_owned();
    let stream = TcpStream::connect(&addr).expect("connect");
    // 30 s, far past the 2 s deadline, so a cut at the client's own timeout can
    // never be mistaken for the server's.
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .expect("client read timeout");
    // The socket stays open and says nothing at all.
    let mut reader = stream.try_clone().expect("clone socket");
    let started = Instant::now();
    let response = thread::spawn(move || {
        let mut raw = Vec::new();
        let mut buf = [0u8; 1024];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => raw.extend_from_slice(&buf[..n]),
            }
        }
        raw
    });
    let text = String::from_utf8_lossy(&response.join().expect("reader")).into_owned();
    let elapsed = started.elapsed();
    let status = status_of(&text);
    assert_eq!(
        status, 408,
        "an idle connection must be answered 408: {text}"
    );
    assert!(
        text.contains("request deadline exceeded (2 s per request)"),
        "must name the deadline the server enforced: {text}"
    );
    // 15 s sits far past the 2 s deadline and far short of the 30 s client read
    // timeout, so a loaded host cannot make this flake and a client-side
    // timeout cannot pass for the server's own cut.
    assert!(
        elapsed < Duration::from_secs(15),
        "the 408 must arrive at the 2 s deadline, not at the client timeout: took {elapsed:?}"
    );
    server.stop();
}
