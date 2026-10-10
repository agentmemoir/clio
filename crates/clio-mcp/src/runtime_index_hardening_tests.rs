// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the guarded runtime-sweeper logging check
//! that proves failed drain reports expose safe metadata without memory text.
//!
//! ## Owns
//! - A subprocess stderr capture around the real index sweeper failure path
//! - A failing provider that echoes the unique content phrase used for redaction proof
//!
//! ## Does not own
//! - Runtime sweeper scheduling (`runtime_index`)
//! - Index worker retry or embedding transport behavior
//!
//! ## Boundary
//!
//! Changes to guarded drain-log regression coverage should normally require changes here.
//! Changes to production logging or drain policy should not require changes here.
//!
//! Keep this module focused on the isolated drain-log hardening test.

use crate::runtime::{McpOpenOptions, McpState};
use clio_types::{EpistemicKind, ItemKind, MemoryItem, SemanticCategory};
use serde_json::json;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::Command;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const CAPTURE_ENV: &str = "CLIO_MCP_INDEX_DRAIN_LOG_CAPTURE";
const TEST_NAME: &str =
    "runtime_index::runtime_index_hardening_tests::drain_failure_log_omits_memory_text";
const CONTENT_PHRASE: &str = "mcp-drain-content-7f3c9a1e";

fn failing_embedder() -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind failing embedder");
    let address = listener.local_addr().expect("failing embedder address");
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept failing embedder request");
        let mut request = [0_u8; 4096];
        let _ = stream.read(&mut request);
        let body = format!("provider echoed {CONTENT_PHRASE}");
        let response = format!(
            "HTTP/1.1 500 Internal Server Error\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream
            .write_all(response.as_bytes())
            .expect("write failing embedder response");
        stream.flush().expect("flush failing embedder response");
    });
    (format!("http://{address}"), server)
}

fn failure_item(bank: &str, id: &str) -> MemoryItem {
    MemoryItem {
        id: id.to_owned(),
        bank_id: bank.to_owned(),
        subject_id: id.to_owned(),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::Schema),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        admission_score: None,
        source_ref: None,
        snapshot: Some(json!({"entity": CONTENT_PHRASE})),
        gist: Some(format!("{CONTENT_PHRASE} must not reach drain logs")),
        valid_from: "t0".into(),
        valid_until: None,
        tx_from: "t0".into(),
        tx_until: None,
        created_at: "t0".into(),
        updated_at: "t0".into(),
        context: None,
    }
}

fn run_failure_scenario() {
    clio_config::log::set_level(clio_config::log::LogLevel::Warn);
    let bank = format!("mcp-log-bank-{}", std::process::id());
    let id = format!("mcp-log-item-{}", std::process::id());
    let (embedder_url, embedder) = failing_embedder();
    let effective = json!({
        "embed": {
            "url": embedder_url,
            "model": "BAAI/bge-small-en-v1.5",
            "dims": 384,
            "provider": "tei"
        }
    });
    let state = McpState::open_with_effective(&McpOpenOptions::default(), &effective)
        .expect("open dead-embedder state");
    state
        .store
        .create_memory_item(&failure_item(&bank, &id), "agent")
        .expect("seed failed index job");
    let sweeper = state.spawn_index_sweeper_with(Duration::from_millis(5));
    state.nudge_index();

    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        let status = state
            .index_status_json(&bank)
            .expect("read failed-job status");
        if status["retry_jobs"] == json!(1) && status["coverage_last_error"].is_string() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "dead embedder did not record a retry before the deadline"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    drop(state);
    sweeper.join().expect("join failure sweeper");
    embedder.join().expect("join failing embedder");
}

#[test]
fn default_sweeper_accessor_starts_and_stops() {
    let state = McpState::open_with_effective(&McpOpenOptions::default(), &json!({}))
        .expect("open default-sweeper state");
    let sweeper = state.spawn_index_sweeper();
    drop(state);
    sweeper.join().expect("join default sweeper");
}

#[test]
fn drain_failure_log_omits_memory_text() {
    if std::env::var_os(CAPTURE_ENV).is_some() {
        run_failure_scenario();
        return;
    }

    let executable = std::env::current_exe().expect("locate current test executable");
    let output = Command::new(executable)
        .args(["--exact", TEST_NAME, "--nocapture"])
        .env(CAPTURE_ENV, "1")
        .output()
        .expect("run guarded log-capture subprocess");
    assert!(
        output.status.success(),
        "guarded log-capture subprocess failed"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    let warning = stderr
        .lines()
        .filter(|line| line.contains("index_drain_failed"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!warning.is_empty(), "drain failure warning was not emitted");
    assert!(warning.contains("failed=1"), "warning lacks failure count");
    assert!(
        warning.contains("error=index_job_failed"),
        "warning lacks the stable safe error label"
    );
    assert!(
        !warning.contains(CONTENT_PHRASE),
        "drain warning exposed memory content"
    );
    assert!(
        !stderr.contains(CONTENT_PHRASE),
        "captured drain output exposed memory content"
    );
}
