// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module drives the real fetcher's transport against a local HTTP server: the
//! redirect chain, the status mapping, the header the advertised size is read from,
//! and the length rule that decides whether what arrived becomes the model file.
//!
//! ## Owns
//! - The fake Hub: a bounded accept loop on a port the OS assigns, one reply per
//!   connection, so the exact number of requests is observable
//! - The transport behaviour under test for `RealFetcher::fetch_to`
//!
//! ## Does not own
//! - The presence and refusal decisions above the fetcher (see
//!   `setup_extract_model_tests`)
//! - The atomic write itself, which is driven with an in-memory reader (see
//!   `setup_extract_model_stream_tests`)
//!
//! ## Boundary
//!
//! Changes to what a download request sends, to how it follows a redirect, or to how
//! it classifies an answer should normally update tests here. Changes to when a fetch
//! runs should not.
//!
//! Keep this module on the socket. Everything above the response is covered where it
//! lives.

use super::*;
use crate::setup_support_tests::temp_dir;
use std::io::{ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// How long the fake Hub waits for a connection it was told to expect. A test that
/// fails before its request arrives leaves the thread with a deadline rather than a
/// listener pinned for the rest of the run.
const SERVE_DEADLINE: Duration = Duration::from_secs(10);

/// How long one accepted connection may take to send its request line.
const REQUEST_READ_TIMEOUT: Duration = Duration::from_secs(5);

/// A redirect answer: no body, and the next hop in `Location`.
///
/// `Connection: close` is what makes one hop one connection, so the accept loop sees
/// a request per hop rather than a reused socket.
fn redirect(status: &str, location: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 {status}\r\nLocation: {location}\r\nContent-Length: 0\r\n\
         Connection: close\r\n\r\n"
    )
    .into_bytes()
}

/// A `200` carrying `body`, advertising `linked` bytes in `x-linked-size`, and
/// stating the transport length the way the Hub does.
fn ok_stating_length(body: &[u8], linked: u64) -> Vec<u8> {
    let mut response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\n\
         x-linked-size: {linked}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    response.extend_from_slice(body);
    response
}

/// A `200` whose body is delimited by the connection closing rather than by a stated
/// length, which is the only way a body longer than `linked` reaches the client at
/// all: a stated length would cut it short in the transport before any check here.
fn ok_without_length(body: &[u8], linked: u64) -> Vec<u8> {
    let mut response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\n\
         x-linked-size: {linked}\r\nConnection: close\r\n\r\n"
    )
    .into_bytes();
    response.extend_from_slice(body);
    response
}

/// A status with no body: the answer an address that has nothing to give sends.
fn status_only(status: &str) -> Vec<u8> {
    format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").into_bytes()
}

/// A fake Hub: one scripted reply per connection, on a port the OS assigns, served by
/// a thread that ends when the script runs out.
struct FakeHub {
    /// The address the fetcher is pointed at, without a path.
    url: String,
    /// The thread answering the script; joined by [`FakeHub::served`].
    handle: JoinHandle<Vec<String>>,
}

impl FakeHub {
    /// Answer `replies` in order, one per connection, then stop. Never serves more
    /// than it was given, so an unexpected extra request by the fetcher fails the
    /// test instead of being answered.
    fn start(replies: Vec<Vec<u8>>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind the fake Hub");
        listener
            .set_nonblocking(true)
            .expect("non-blocking fake Hub");
        let url = format!(
            "http://127.0.0.1:{}",
            listener.local_addr().expect("fake Hub address").port()
        );
        let handle = thread::spawn(move || {
            let deadline = Instant::now() + SERVE_DEADLINE;
            let mut answered = Vec::new();
            for reply in replies {
                let Some(mut stream) = accept_by(&listener, deadline) else {
                    break;
                };
                answered.push(request_target(&mut stream));
                let _ = stream.write_all(&reply);
                let _ = stream.flush();
                // The close on drop is what ends a body with no stated length.
                drop(stream);
            }
            answered
        });
        Self { url, handle }
    }

    /// The url of one path on this server.
    fn path(&self, path: &str) -> String {
        format!("{}{path}", self.url)
    }

    /// Join the thread and return the paths it answered, in arrival order.
    fn served(self) -> Vec<String> {
        self.handle.join().expect("the fake Hub thread panicked")
    }
}

/// Accept one connection, or give up at `deadline` so no test can wait forever.
fn accept_by(listener: &TcpListener, deadline: Instant) -> Option<TcpStream> {
    loop {
        match listener.accept() {
            Ok((stream, _)) => return Some(stream),
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return None;
                }
                thread::sleep(Duration::from_millis(1));
            }
            Err(_) => return None,
        }
    }
}

/// The path one connection asked for, read off its request line.
fn request_target(stream: &mut TcpStream) -> String {
    stream
        .set_nonblocking(false)
        .expect("blocking fake Hub stream");
    stream
        .set_read_timeout(Some(REQUEST_READ_TIMEOUT))
        .expect("fake Hub read timeout");
    let mut raw = Vec::new();
    let mut chunk = [0_u8; 1_024];
    while !raw.windows(2).any(|window| window.starts_with(b"\r\n")) {
        let read = stream.read(&mut chunk).unwrap_or(0);
        if read == 0 {
            break;
        }
        raw.extend_from_slice(&chunk[..read]);
    }
    String::from_utf8_lossy(&raw)
        .split_whitespace()
        .nth(1)
        .unwrap_or_default()
        .to_owned()
}

#[test]
fn a_redirect_chain_is_followed_and_the_body_lands_at_the_destination() {
    let dir = temp_dir("http-redirects");
    let dest = dir.join(GGUF_FILE_NAME);
    let body = b"tiny-model-bytes";
    let linked = body.len() as u64;
    let hub = FakeHub::start(vec![
        redirect("307 Temporary Redirect", "/second"),
        redirect("302 Found", "/third"),
        ok_stating_length(body, linked),
    ]);
    let mut progress = Vec::new();

    let written = RealFetcher
        .fetch_to(&hub.path("/first"), &dest, &mut |bytes| {
            progress.push(bytes);
        })
        .expect("a chain that ends in a whole body is a download");

    assert_eq!(
        hub.served(),
        vec!["/first", "/second", "/third"],
        "one request per hop, in the order the chain names"
    );
    assert_eq!(written, linked);
    assert_eq!(progress, vec![linked], "a small body reports progress once");
    assert_eq!(
        std::fs::read(&dest).expect("the model is on disk"),
        body,
        "the body the last hop sent is the body on disk"
    );
    assert!(
        !part_path(&dest).exists(),
        "the partial file was renamed into place, not left behind"
    );
}

#[test]
fn a_404_answer_is_refused_by_status_and_leaves_no_file() {
    let dir = temp_dir("http-404");
    let dest = dir.join(GGUF_FILE_NAME);
    let hub = FakeHub::start(vec![status_only("404 Not Found")]);
    let mut progress = Vec::new();

    let err = RealFetcher
        .fetch_to(&hub.path("/missing"), &dest, &mut |bytes| {
            progress.push(bytes);
        })
        .expect_err("a 404 is not a download");

    assert_eq!(hub.served(), vec!["/missing"], "one request, no retry");
    assert!(err.contains("status 404"), "the status is named: {err}");
    assert!(
        progress.is_empty(),
        "nothing is streamed for a refused answer: {progress:?}"
    );
    assert!(!dest.exists(), "a refused answer never becomes the model");
    assert!(
        !part_path(&dest).exists(),
        "no partial file is left for a refused answer"
    );
}

#[test]
fn a_body_shorter_than_the_advertised_size_is_refused_and_leaves_no_file() {
    let dir = temp_dir("http-short");
    let dest = dir.join(GGUF_FILE_NAME);
    let body = vec![b'z'; 500];
    let hub = FakeHub::start(vec![ok_stating_length(&body, 1_000)]);
    let mut progress = Vec::new();

    let err = RealFetcher
        .fetch_to(&hub.path("/short"), &dest, &mut |bytes| {
            progress.push(bytes);
        })
        .expect_err("a body shorter than the advertised size is not a download");

    assert_eq!(hub.served(), vec!["/short"]);
    assert!(err.contains("500 of 1000 bytes"), "{err}");
    assert_eq!(
        progress,
        vec![500],
        "the bytes arrive and are reported before the size is judged"
    );
    assert!(!dest.exists(), "a short body never becomes the model");
    assert!(
        !part_path(&dest).exists(),
        "the partial file is removed when the length does not match"
    );
}

#[test]
fn a_body_larger_than_the_advertised_size_is_refused_by_the_same_check() {
    let dir = temp_dir("http-long");
    let dest = dir.join(GGUF_FILE_NAME);
    let body = vec![b'z'; 900];
    let hub = FakeHub::start(vec![ok_without_length(&body, 100)]);
    let mut progress = Vec::new();

    let err = RealFetcher
        .fetch_to(&hub.path("/long"), &dest, &mut |bytes| {
            progress.push(bytes);
        })
        .expect_err("a body longer than the advertised size is not a download");

    assert_eq!(hub.served(), vec!["/long"]);
    assert!(
        err.contains("900 of 100 bytes"),
        "the length rule refuses an over-long body as well as a short one: {err}"
    );
    assert_eq!(progress, vec![900]);
    assert!(!dest.exists(), "an over-long body never becomes the model");
    assert!(
        !part_path(&dest).exists(),
        "the partial file is removed when the length does not match"
    );
}
