// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the MCP transport's per-request byte and time bounds
//! against the real `serve_with_listener` entry point over real TCP: the
//! 64 KiB head cap, the 10 MiB body cap, and the total request deadline.
//!
//! ## Owns
//! - Socket-level tests proving each bound is enforced and its refusal names
//!   the bound and its value
//! - The slow-drip deadline cut, including the bound that distinguishes the
//!   configured deadline from a merely-bounded read
//! - The head cap boundary from the accepted side: a header block whose
//!   terminator lands on the cap is served, in one write and in two, and a
//!   client that sends nothing is answered a 408 naming the total deadline
//!
//! ## Does not own
//! - The connection cap and slot release (`transport_bounds_tests`)
//! - Pool unit tests (`conn_pool_tests`)
//! - Request parsing and the bound values themselves (`http_read`, `http`)
//!
//! ## Boundary
//!
//! Changes to how a single request's bytes or time are bounded should normally
//! require changes here. Changes to routing, sessions, or connection
//! accounting should not require changes here.
//!
//! Keep this module focused on per-request resource-bound tests. Tests assert
//! outcomes and generous bounds, never precise durations.

use crate::http::{HttpConfig, MAX_BODY, MAX_HEAD_BYTES, serve_with_listener};
use crate::runtime::{McpOpenOptions, McpState};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const INIT: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"0"}}}"#;

/// Serve through the real `serve_with_listener` in a background thread. The
/// thread runs until the per-test process exits, which matches the production
/// loop shape.
fn spawn_server(cfg: HttpConfig) -> u16 {
    let state = McpState::open(&McpOpenOptions::default()).expect("state");
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    std::thread::spawn(move || {
        let _ = serve_with_listener(state, cfg, listener);
    });
    port
}

fn connect(port: u16) -> TcpStream {
    TcpStream::connect(("127.0.0.1", port)).expect("connect")
}

fn init_request() -> String {
    format!(
        "POST /mcp HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\n\
         Accept: application/json\r\nContent-Length: {}\r\n\r\n{INIT}",
        INIT.len()
    )
}

/// Split a raw response into `(status, body)`; `None` without a full head.
fn parse_response(buf: &[u8]) -> Option<(u16, String)> {
    let pos = buf.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = String::from_utf8_lossy(&buf[..pos]);
    let status = head
        .split("\r\n")
        .next()?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()?;
    let body = String::from_utf8_lossy(&buf[pos + 4..]).into_owned();
    Some((status, body))
}

#[test]
fn slow_drip_request_is_cut_at_the_total_deadline() {
    // Generous multiples of the configured deadline, so a loaded host cannot
    // flake them. The cut can never land *before* the deadline by construction:
    // the server only knows the deadline expired, and the client imposes no
    // timeout at all, so the reader blocking until the server closes is the
    // only thing the elapsed time can measure.
    let deadline = Duration::from_secs(2);
    let deadline_multiple = 3;
    let cfg = HttpConfig {
        deadline_secs: Some(2),
        ..HttpConfig::default()
    };
    let port = spawn_server(cfg);
    let mut stream = connect(port);
    // A dedicated reader consumes the response as soon as it arrives, so a
    // later drip write that trips a reset cannot discard the refusal.
    let mut reader = stream.try_clone().expect("clone for reader");
    let reader_handle = std::thread::spawn(move || {
        let mut buf = Vec::new();
        // The server always closes this connection (deadline cut, handled
        // request, or clean EOF), so the blocking read terminates.
        let _ = reader.read_to_end(&mut buf);
        buf
    });
    let req = init_request();
    // Drip one byte at a time. Each drip is far inside the read slice, and
    // the full drip (about 5.3 s at 20 ms per byte) is far past the 2 s
    // deadline, so the cut point is the server's total deadline; scheduler
    // jitter changes how many bytes were sent, never the outcome.
    //
    // The server's clock starts when it accepts the connection, which is
    // before this loop runs, so the drip's own 20 ms of sleep per byte is
    // additive slack on top of the server's deadline rather than time the
    // server could have used to serve a response. It is therefore honest to
    // bound the whole thing from here: a client with no timeout of its own
    // cannot be what ends the drip early.
    let drip_start = Instant::now();
    for byte in req.as_bytes() {
        if stream.write_all(&[*byte]).is_err() {
            break; // the server already cut the connection at the deadline
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(stream);
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(reader_handle.join().expect("reader thread"));
    });
    let buf = rx
        .recv_timeout(Duration::from_secs(15))
        .expect("the server must close the connection at the deadline");
    let elapsed = drip_start.elapsed();
    let (status, body) =
        parse_response(&buf).expect("the deadline cut must produce a readable refusal");
    assert_eq!(
        status, 408,
        "a slow-drip client must be cut at the total deadline"
    );
    assert!(
        body.contains("2 s total request deadline"),
        "the refusal must name the deadline and its value: {body}"
    );
    // Upper bound only, no lower bound: the deadline exists to stop a slow
    // client, so cutting at any point up to this generous multiple is a pass.
    // Without it, any effective deadline between 2 s and the roughly 5.3 s
    // drip would produce the same 408 and this test could not tell them apart.
    assert!(
        elapsed < deadline * deadline_multiple,
        "the cut must land at the configured deadline, not merely before the drip ends: \
         {elapsed:?} elapsed for a {deadline:?} deadline"
    );
}
#[test]
fn declared_body_over_cap_rejected_before_body_arrives() {
    let port = spawn_server(HttpConfig::default());
    let mut stream = connect(port);
    let head = format!(
        "POST /mcp HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\n\
         Accept: application/json\r\nContent-Length: {}\r\n\r\n",
        MAX_BODY + 1
    );
    stream.write_all(head.as_bytes()).expect("write head");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("read timeout");
    let mut buf = Vec::new();
    let _ = stream.read_to_end(&mut buf); // no body bytes are ever sent
    let (status, body) = parse_response(&buf).expect("an oversize declared body must be refused");
    assert_eq!(status, 413);
    assert!(
        body.contains("10 MiB body cap"),
        "the refusal must name the body cap and its value: {body}"
    );
}
#[test]
fn header_block_over_head_cap_rejected() {
    let port = spawn_server(HttpConfig::default());
    let mut stream = connect(port);
    // A header block past the 64 KiB cap with no terminator: the cap must trip
    // while the head is still growing.
    let head = format!("POST /mcp HTTP/1.1\r\nX-Pad: {}\r\n", "a".repeat(70 * 1024));
    let _ = stream.write_all(head.as_bytes());
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("read timeout");
    let mut buf = Vec::new();
    let _ = stream.read_to_end(&mut buf);
    let (status, body) = parse_response(&buf).expect("an oversize header block must be refused");
    assert_eq!(status, 413);
    assert!(
        body.contains("64 KiB head cap"),
        "the refusal must name the head cap and its value: {body}"
    );
}

/// One `initialize` request whose whole header block, terminator included, is
/// exactly `MAX_HEAD_BYTES`. The filler absorbs the length of everything else,
/// so the total is exact by construction rather than by arithmetic a later edit
/// could drift from, and independent of the port the OS assigns.
fn head_on_the_cap() -> String {
    let bare = format!(
        "POST /mcp HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\n\
         Accept: application/json\r\nContent-Length: {}\r\nX-Filler: \r\n\r\n",
        INIT.len()
    );
    let filler = "a".repeat(MAX_HEAD_BYTES - bare.len());
    bare.replace("X-Filler: \r\n", &format!("X-Filler: {filler}\r\n"))
}

/// Send one request as two writes with a pause between them, so the read that
/// carries the header terminator also carries bytes past the cap.
///
/// A single write cannot reach that case. The reads then land on multiples of
/// the 8192-byte read size, so the read that completes the head lands exactly on
/// the cap, and a cap judged on the pre-append buffer still serves it. Letting
/// the first half drain first is what puts the buffer at an offset that is not
/// a multiple of the read size, which is the only way the terminating read
/// crosses the cap. The pause is orders of magnitude above the loopback cost of
/// draining 62 KB, and the two halves landing in one read would leave the test
/// passing without reaching the case, so the split is asserted below rather
/// than assumed.
fn exchange_split(port: u16, first: &str, second: &[u8]) -> Option<(u16, String)> {
    let mut stream = connect(port);
    // Both directions are bounded, so a server that never answers fails this
    // test instead of blocking the test thread forever.
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .expect("read timeout");
    stream
        .set_write_timeout(Some(Duration::from_secs(30)))
        .expect("write timeout");
    stream.write_all(first.as_bytes()).expect("first half");
    stream.flush().expect("flush first half");
    std::thread::sleep(Duration::from_millis(200));
    stream.write_all(second).expect("second half");
    stream.flush().expect("flush second half");
    let mut buf = Vec::new();
    let _ = stream.read_to_end(&mut buf);
    parse_response(&buf)
}

#[test]
fn header_block_exactly_on_head_cap_is_served() {
    let port = spawn_server(HttpConfig::default());
    let head = head_on_the_cap();
    assert_eq!(
        head.len(),
        MAX_HEAD_BYTES,
        "the head must sit exactly on the cap"
    );

    // Shape one: head and body in a single write, so the terminator lands
    // exactly on the cap.
    let mut stream = connect(port);
    stream.write_all(head.as_bytes()).expect("write head");
    stream.write_all(INIT.as_bytes()).expect("write body");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("read timeout");
    let mut buf = Vec::new();
    let _ = stream.read_to_end(&mut buf);
    let (status, body) = parse_response(&buf).unwrap_or_else(|| {
        panic!(
            "a head on the cap must be served, got {:?}",
            String::from_utf8_lossy(&buf)
        )
    });
    assert_eq!(
        status, 200,
        "a header block that fits inside the cap must never be refused: {body}"
    );

    // Shape two: the same block in two writes, so the read carrying the
    // terminator also carries the body past the cap. Judging the head before
    // appending that chunk refuses this shape with 413 even though the block
    // ends exactly on the cap, which is the defect this test pins.
    let (first, second) = head.split_at(62_000);
    assert!(
        !first.contains("\r\n\r\n"),
        "the first half must hold no terminator, or the split proves nothing"
    );
    let mut tail = second.as_bytes().to_vec();
    tail.extend_from_slice(INIT.as_bytes());
    let (status, body) = exchange_split(port, first, &tail)
        .expect("a head on the cap must be answered, not silently dropped");
    assert_eq!(
        status, 200,
        "a head that ends on the cap must be served when its terminator arrives in the \
         read that crosses the cap: {body}"
    );
}

#[test]
fn idle_connection_is_cut_at_the_total_deadline() {
    let cfg = HttpConfig {
        deadline_secs: Some(2),
        ..HttpConfig::default()
    };
    let port = spawn_server(cfg);
    // The client sends nothing at all. Nothing here is dripped or withheld
    // part-way, so the only thing that can end this connection is the server's
    // own total deadline. `stream` stays open for the whole test: dropping it
    // would close the client side, and the server would then see a clean EOF
    // and answer nothing at all.
    let stream = connect(port);
    // The socket read timeout is the client's only bound. It also makes
    // `join()` below unable to hang: a read that never returns bytes fails with
    // this timeout and the thread ends.
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .expect("read timeout");
    let mut reader = stream.try_clone().expect("clone for reader");
    let start = Instant::now();
    let reader_handle = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = reader.read_to_end(&mut buf);
        buf
    });
    let buf = reader_handle.join().expect("reader thread");
    let elapsed = start.elapsed();
    // A missing response fails here. The elapsed bound below then covers the
    // other failure mode: a refusal that arrives far later than the deadline.
    let (status, body) = parse_response(&buf).expect("the silent client must get a 408 refusal");
    assert_eq!(
        status, 408,
        "a client that sends nothing must be cut at the total deadline"
    );
    assert!(
        body.contains("2 s total request deadline"),
        "the refusal must name the deadline and its value, proving the server's own \
         deadline produced it: {body}"
    );
    // Generous bound only, no lower bound, and far apart from both ends it has
    // to tell apart: 15 s is past the 2 s deadline by a wide margin, so a loaded
    // host cannot push a correct cut past it, and far short of the 30 s client
    // read timeout, so a cut that only happens because the client gave up
    // cannot hide under it.
    assert!(
        elapsed < Duration::from_secs(15),
        "the cut must land at the configured deadline, not at the client's read timeout: \
         {elapsed:?} elapsed for a 2 s deadline"
    );
    drop(stream);
}
