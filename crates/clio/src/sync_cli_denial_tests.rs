// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `clio sync serve` process's erasure-denial wiring: a
//! deployment whose journal path resolves serves through the denial authority
//! (a denied bank is refused, an unrelated bank still serves), and a deployment
//! with no resolvable journal path still starts and serves as before.
//!
//! ## Owns
//! - Behavior coverage for the serve process's denial-aware branch
//!
//! ## Does not own
//! - The server's own denial behavior (`clio-sync` `server_denial_tests`)
//! - The verb decoding and refusal shapes (`sync_cli_tests`)
//!
//! ## Boundary
//!
//! These tests break when the serve process's authority wiring changes; they
//! never change it.
//!
//! Keep this module focused on the serve process's denial wiring.

use super::*;
use crate::sync_cli::sync_cli_serve::start_serve;
use clio_compliance::JournalDenialProbe;
use clio_store::ErasureDenial;
use clio_sync::denial::denied_bank_marker;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// One scratch journal path, removed on drop: the crate's fixture pattern —
/// the pid and a nanosecond stamp keep parallel tests apart.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let file = format!("am-sync-cli-{name}-{}-{nanos}.json", std::process::id());
        Self(std::env::temp_dir().join(file))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// One pull of `bank`; returns the status and the decoded body.
fn pull(addr: &str, bank: &str) -> (u16, Value) {
    let body = serde_json::to_vec(&json!({ "bank": bank, "cursor": "", "limit": 10 }))
        .expect("serialize pull");
    let head = format!(
        "POST /sync/pull HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let mut stream = TcpStream::connect(addr).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .expect("read timeout");
    stream
        .write_all(head.as_bytes())
        .and_then(|()| stream.write_all(&body))
        .expect("send the pull");
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).expect("read the response");
    let text = String::from_utf8_lossy(&raw);
    let status: u16 = text
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let body_start = text.find("\r\n\r\n").map_or(text.len(), |p| p + 4);
    let value = serde_json::from_str(text[body_start..].trim()).expect("decode the body");
    (status, value)
}

/// The serve arguments one `--dev --bind 127.0.0.1:0 --banks b-denied,b-open`
/// command line decodes to, built through the verb's own decoder.
fn serve_args(banks: &str) -> Value {
    let parsed = parsed_args(
        "sync serve",
        &["--dev", "--bind", "127.0.0.1:0", "--banks", banks],
    );
    let (tool, args) = build("sync serve", &parsed, &|_| None).expect("build");
    assert_eq!(tool, "sync_serve");
    args
}

/// A resolved journal path serves through the denial authority: a bank a
/// recorded denial covers is refused with the erased-subject reason while an
/// unrelated bank still serves. The server is started through the same
/// function the `sync serve` verb runs, so this pins the production branch.
#[test]
fn a_configured_journal_refuses_a_denied_bank_and_serves_the_rest() {
    let journal = Scratch::new("serve-denial");
    JournalDenialProbe::new(journal.path().to_path_buf())
        .record_denial(&denied_bank_marker("b-denied"), &["b-denied".to_owned()])
        .expect("record the bank denial");
    let get = |key: &str| {
        (key == clio_config::ERASURE_JOURNAL_FILE_ENV)
            .then(|| journal.path().to_string_lossy().into_owned())
    };
    let (server, payload) = start_serve(&state(), &serve_args("b-denied,b-open"), &get)
        .expect("the serve process starts");
    assert!(payload["addr"].as_str().is_some(), "{payload}");

    let (status, value) = pull(server.addr(), "b-denied");
    assert_eq!(status, 403, "{value}");
    assert_eq!(value["code"], "erased_subject", "{value}");
    let (status, value) = pull(server.addr(), "b-open");
    assert_eq!(status, 200, "an unrelated bank still serves: {value}");
    server.stop();
}

/// No resolvable journal path is the pre-denial deployment: the serve process
/// still starts and serves the same banks, with no authority refusal.
#[test]
fn no_resolvable_journal_path_still_starts_and_serves() {
    let env = |_key: &str| -> Option<String> { None };
    let (server, payload) = start_serve(&state(), &serve_args("b-denied,b-open"), &env)
        .expect("the serve process starts without a journal");
    assert!(payload["addr"].as_str().is_some(), "{payload}");
    let (status, value) = pull(server.addr(), "b-denied");
    assert_eq!(status, 200, "no authority wired: {value}");
    server.stop();
}
