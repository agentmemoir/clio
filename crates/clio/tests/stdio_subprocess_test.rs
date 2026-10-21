// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module drives the real `clio mcp stdio` binary over OS pipes: it sends
//! store, status, and retrieve frames and asserts the stored item becomes
//! searchable through the background index sweeper.
//!
//! ## Owns
//! - A subprocess launch of the compiled `clio` binary with piped stdin/stdout
//! - End-to-end store → sweeper drain → retrieve evidence over real pipes
//!
//! ## Does not own
//! - In-process stdio framing tests (`clio-mcp::stdio_tests`)
//! - CLI flag parsing or database resolution (`clio-config`/`clio`)
//!
//! ## Boundary
//!
//! Changes to the `clio mcp stdio` process lifecycle should normally require
//! changes here. Changes to MCP tool semantics should not require changes here.
//!
//! Keep this module focused on the subprocess stdio lifecycle.

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// Longest we wait for one MCP response before failing the test.
const RECV_TIMEOUT: Duration = Duration::from_secs(10);
/// Longest we let the background sweeper take to drain the outbox.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(8);
/// Longest we wait for the server to exit after its stdin closes.
const EXIT_TIMEOUT: Duration = Duration::from_secs(10);
/// Shortest pause between exit polls.
const EXIT_POLL: Duration = Duration::from_millis(20);
/// Longest we wait for a killed child to be reaped.
const REAP_TIMEOUT: Duration = Duration::from_secs(2);

/// Kills the server on drop so a panicking assertion cannot orphan it.
struct ServerGuard(Child);

impl Drop for ServerGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// The live parts of a spawned server: process guard, stdin, response channel,
/// captured stderr, and the stdout and stderr reader threads.
type ServerParts = (
    ServerGuard,
    ChildStdin,
    Receiver<String>,
    Arc<Mutex<String>>,
    thread::JoinHandle<()>,
    thread::JoinHandle<std::io::Result<()>>,
);

/// Launch `clio mcp stdio` over a volatile in-memory database.
fn spawn_server() -> ServerParts {
    let mut child = Command::new(env!("CARGO_BIN_EXE_clio"))
        .args(["mcp", "stdio", "--db", "sqlite::memory:"])
        // Force an empty deployment overlay so the scenario never reads a host
        // sidecar config; an empty value is not used because that disables
        // deployment loading entirely.
        .env(
            "CLIO_DEPLOYMENT_CONFIG",
            std::env::temp_dir().join("clio-stdio-subprocess-absent-overlay.json"),
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn clio mcp stdio");
    let stdin = child.stdin.take().expect("child stdin");
    let stdout = child.stdout.take().expect("child stdout");
    let stderr = child.stderr.take().expect("child stderr");

    let (sender, receiver) = mpsc::channel();
    let stdout_reader = thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            match line {
                Ok(line) => {
                    if sender.send(line).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
    let captured = Arc::new(Mutex::new(String::new()));
    let sink = Arc::clone(&captured);
    let stderr_reader = thread::spawn(move || -> std::io::Result<()> {
        let mut reader = BufReader::new(stderr);
        let mut line = String::new();
        loop {
            let read = reader.read_line(&mut line)?;
            if read == 0 {
                return Ok(());
            }
            if let Ok(mut guard) = sink.lock() {
                guard.push_str(&line);
            }
            line.clear();
        }
    });
    (
        ServerGuard(child),
        stdin,
        receiver,
        captured,
        stdout_reader,
        stderr_reader,
    )
}

/// Join one reader thread, surfacing its read failure and its panic where the
/// stream was read, instead of leaving the cause to surface later as a short
/// capture.
fn join_reader(handle: thread::JoinHandle<std::io::Result<()>>, stream: &str) {
    match handle.join() {
        Ok(Ok(())) => {}
        Ok(Err(error)) => panic!("read the clio mcp stdio {stream}: {error}"),
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

/// Write one compact JSON-RPC frame and flush it.
fn send(stdin: &mut ChildStdin, frame: &Value) {
    writeln!(stdin, "{frame}").expect("write frame");
    stdin.flush().expect("flush frame");
}

/// Read exactly one response line, failing with captured stderr on timeout.
fn recv_response(receiver: &Receiver<String>, stderr: &Arc<Mutex<String>>) -> Value {
    match receiver.recv_timeout(RECV_TIMEOUT) {
        Ok(line) => serde_json::from_str(&line).expect("single-line JSON-RPC response"),
        Err(error) => {
            let captured = stderr
                .lock()
                .map(|text| text.to_string())
                .unwrap_or_default();
            panic!("no MCP response within {RECV_TIMEOUT:?}: {error}; stderr: {captured}");
        }
    }
}

/// Wait for the server to exit, killing and reaping it when it does not exit
/// before `budget`. `Err` reports what the termination actually did.
fn wait_for_exit(child: &mut Child, budget: Duration) -> Result<std::process::ExitStatus, String> {
    let deadline = Instant::now() + budget;
    loop {
        match child.try_wait().expect("reap the clio mcp stdio child") {
            Some(status) => return Ok(status),
            None if Instant::now() < deadline => thread::sleep(EXIT_POLL),
            None => return Err(terminate(child, budget)),
        }
    }
}

/// Kill a child that missed its deadline and reap it within a bound, wording
/// the outcome from what the calls actually returned.
fn terminate(child: &mut Child, budget: Duration) -> String {
    if let Err(error) = child.kill() {
        return format!(
            "clio mcp stdio did not exit within {budget:?} and the kill failed: {error}"
        );
    }
    let deadline = Instant::now() + REAP_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                return format!("clio mcp stdio did not exit within {budget:?} and was killed");
            }
            Ok(None) if Instant::now() < deadline => thread::sleep(EXIT_POLL),
            Ok(None) => {
                return format!(
                    "clio mcp stdio did not exit within {budget:?} and was killed but not reaped within {REAP_TIMEOUT:?}"
                );
            }
            Err(error) => {
                return format!(
                    "clio mcp stdio did not exit within {budget:?} and was killed, but the reap failed: {error}"
                );
            }
        }
    }
}

#[allow(clippy::needless_pass_by_value)]
fn call(id: i64, name: &str, arguments: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "tools/call",
        "params": {"name": name, "arguments": arguments}
    })
}

/// The real binary serves store → drain → retrieve over OS pipes, so the
/// background sweeper thread is exercised, not a test-only in-memory reader.
#[test]
fn mcp_stdio_subprocess_store_drains_and_retrieves() {
    let (mut guard, mut stdin, receiver, stderr, stdout_reader, stderr_reader) = spawn_server();
    let bank = "bank-subprocess";
    let item_id = "itm-subprocess-1";

    let store = call(
        1,
        "store",
        json!({
            "item": {
                "id": item_id,
                "subject_id": item_id,
                "gist": "prefers terse answers",
                "snapshot": {
                    "entity": "terse answers",
                    "amount": 1,
                    "date": "2024-01-01"
                }
            },
            "category": "persona",
            "epistemic_kind": "fact",
            "source_text": "The user prefers terse answers (1 note since 2024-01-01).",
            "bank": bank,
            "actor": "agent"
        }),
    );
    send(&mut stdin, &store);
    let stored = recv_response(&receiver, &stderr);
    assert_eq!(stored["result"]["isError"], json!(false), "{stored}");
    assert_eq!(
        stored["result"]["structuredContent"]["id"],
        json!(item_id),
        "{stored}"
    );

    // The write path only nudges the sweeper; poll the live status over the same
    // pipes until the background thread has drained the durable outbox.
    let deadline = Instant::now() + DRAIN_TIMEOUT;
    let mut next_id = 2;
    loop {
        send(
            &mut stdin,
            &call(next_id, "maintenance_status", json!({"bank": bank})),
        );
        let status = recv_response(&receiver, &stderr);
        let index = &status["result"]["structuredContent"]["index"];
        if index["pending_jobs"] == json!(0) && index["lexical_rows"] == json!(1) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "background sweeper did not drain the outbox: {status}"
        );
        thread::sleep(Duration::from_millis(25));
        next_id += 1;
    }

    send(
        &mut stdin,
        &call(
            next_id + 1,
            "retrieve",
            json!({"bank": bank, "query": "terse answers", "limit": 5}),
        ),
    );
    let retrieved = recv_response(&receiver, &stderr);
    assert_eq!(retrieved["result"]["isError"], json!(false), "{retrieved}");
    let hits = retrieved["result"]["structuredContent"]["hits"]
        .as_array()
        .expect("retrieve hits");
    assert!(
        hits.iter().any(|hit| hit["item_id"] == json!(item_id)),
        "stored id missing from subprocess retrieve: {retrieved}"
    );

    // Closing stdin is EOF for the session; the binary must exit cleanly.
    drop(stdin);
    let exit = wait_for_exit(&mut guard.0, EXIT_TIMEOUT);
    stdout_reader
        .join()
        .expect("the MCP stdout reader thread panicked");
    join_reader(stderr_reader, "stderr");
    let captured = stderr
        .lock()
        .map(|text| text.to_string())
        .unwrap_or_default();
    let status = exit.unwrap_or_else(|failure| panic!("{failure}; stderr: {captured}"));
    assert!(
        status.success(),
        "clio mcp stdio exited {status:?}: {captured}"
    );
}

/// A child that never sees its shutdown frame must hit the exit deadline, and
/// the wait helper must kill, reap, and say so. The guard reaps it on unwind
/// even if this test fails before the deadline branch runs.
#[test]
#[should_panic(expected = "did not exit within 200ms and was killed")]
fn mcp_stdio_subprocess_reports_the_exit_deadline_it_enforces() {
    // `_stdin` stays bound for the whole test, so the child keeps waiting.
    let (mut guard, _stdin, _receiver, stderr, stdout_reader, stderr_reader) = spawn_server();
    let failure = wait_for_exit(&mut guard.0, Duration::from_millis(200))
        .expect_err("a child that never sees shutdown must miss the deadline");
    stdout_reader
        .join()
        .expect("the MCP stdout reader thread panicked");
    join_reader(stderr_reader, "stderr");
    let captured = stderr
        .lock()
        .map(|text| text.to_string())
        .unwrap_or_default();
    panic!("{failure}; stderr: {captured}");
}
