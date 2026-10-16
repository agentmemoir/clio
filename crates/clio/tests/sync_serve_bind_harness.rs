// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for process-level contract tests of
//! `clio sync serve` startup: the non-loopback bind warning on stderr in dev
//! mode and with a credential store, its absence on a loopback bind, and stdout
//! purity under `--output json`.
//!
//! ## Owns
//! - Spawning the real `clio` binary for `sync serve` and asserting its
//!   startup streams
//!
//! ## Does not own
//! - The warning text itself (`clio-sync` `bind_warning_tests`)
//! - The documented field sets (`clio-sync` `client_feed_documentation_tests`)
//! - Transport protocol behavior after startup
//! - The cleared environment, scratch credential store, and erasure journal a
//!   normal-mode start loads (`sync_wire_support` supplies them)
//!
//! ## Boundary
//!
//! Tests here break when the CLI startup contract changes; they never change
//! it.
//!
//! Keep this module focused on the process-level sync serve startup contract.

use std::io::{BufRead, BufReader, Read};
use std::process::{Child, Stdio};
use std::sync::mpsc;
use std::time::Duration;

#[path = "sync_wire_support/mod.rs"]
mod sync_wire_support;
use sync_wire_support::{NodeDir, clio, write_credential_store};

/// The warning prefix the sync server writes to stderr on a public bind.
const WARNING_PREFIX: &str = "sync serve: bind ";
/// The line `clio sync serve` prints once the server is up.
const LISTENING_PREFIX: &str = "sync serve listening on ";

/// Spawn `clio sync serve` with the test's own scratch directory, so every
/// writable path a start resolves — the erasure journal a serve start refuses
/// to bind without, among them — stays inside this test's scratch directory.
fn spawn_serve(scratch: &NodeDir, extra: &[&str]) -> Child {
    let mut args = vec!["sync", "serve"];
    args.extend_from_slice(extra);
    clio(scratch.path())
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn clio sync serve")
}

/// Read a child's stderr until a line carrying `needle` arrives, with a bound.
/// Returns everything read so far when the child dies or the bound passes.
fn stderr_until(child: &mut Child, needle: &str) -> String {
    let stderr = child.stderr.take().expect("stderr piped");
    let needle = needle.to_owned();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut seen = String::new();
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            seen.push_str(&line);
            seen.push('\n');
            if line.contains(&needle) {
                break;
            }
        }
        let _ = tx.send(seen);
    });
    rx.recv_timeout(Duration::from_secs(20)).unwrap_or_default()
}

/// Wait for the listening line on a child's stdout, with a bound.
fn stdout_listening(child: &mut Child) -> bool {
    let stdout = child.stdout.take().expect("stdout piped");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if line.starts_with(LISTENING_PREFIX) {
                let _ = tx.send(true);
                return;
            }
        }
        let _ = tx.send(false);
    });
    rx.recv_timeout(Duration::from_secs(20)).unwrap_or(false)
}

/// Read a child's stdout until a line carrying `needle` arrives, with a bound.
/// Returns everything read so far when the child dies or the bound passes.
fn stdout_until(child: &mut Child, needle: &str) -> String {
    let stdout = child.stdout.take().expect("stdout piped");
    let needle = needle.to_owned();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut seen = String::new();
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            seen.push_str(&line);
            seen.push('\n');
            if line.contains(&needle) {
                break;
            }
        }
        let _ = tx.send(seen);
    });
    rx.recv_timeout(Duration::from_secs(20)).unwrap_or_default()
}

/// Kill the child and return everything it wrote to stdout.
fn kill_and_collect_stdout(mut child: Child) -> Vec<u8> {
    let _ = child.kill();
    let mut bytes = Vec::new();
    if let Some(mut stream) = child.stdout.take() {
        let _ = stream.read_to_end(&mut bytes);
    }
    let _ = child.wait();
    bytes
}

/// Kill the child and return everything it wrote to stderr.
fn kill_and_collect_stderr(mut child: Child) -> Vec<u8> {
    let _ = child.kill();
    let mut bytes = Vec::new();
    if let Some(mut stream) = child.stderr.take() {
        let _ = stream.read_to_end(&mut bytes);
    }
    let _ = child.wait();
    bytes
}

/// A public bind warns on stderr — naming the routing fields, the fields that
/// cross in the clear in a keyed form, the sealed pair, and the residual — in dev
/// mode, and stdout stays a parseable json document. Which side of the line each
/// name sits on is checked against the shared table, not by substring presence;
/// this harness checks that a real binary's stderr carries the names at all.
#[test]
fn a_public_dev_bind_warns_on_stderr_and_keeps_stdout_json() {
    let scratch = NodeDir::new("bind-public-dev");
    let mut child = spawn_serve(
        &scratch,
        &[
            "--bind",
            "0.0.0.0:0",
            "--dev",
            "--ttl-secs",
            "30",
            "--output",
            "json",
            "--db",
            "sqlite::memory:",
        ],
    );
    let stderr = stderr_until(&mut child, WARNING_PREFIX);
    assert!(
        stderr.contains(WARNING_PREFIX),
        "no warning on a public bind: {stderr:?}"
    );
    for field in [
        "bank scope",
        "record kind",
        "operation",
        "transaction time",
        "device id",
        "record-kind mix",
        "device-id stability",
        "record identifier",
        "content digest",
        "admission score",
    ] {
        assert!(
            stderr.contains(field),
            "warning missing `{field}`: {stderr:?}"
        );
    }
    // The warning is written before the tool answers, so wait for the answer
    // instead of racing the kill against it.
    let stdout = stdout_until(&mut child, "addr");
    let _ = kill_and_collect_stderr(child);
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("stdout must parse as json: {e}: {stdout:?}"));
    assert!(
        parsed
            .get("addr")
            .and_then(|a| a.as_str())
            .unwrap_or("")
            .starts_with("0.0.0.0:"),
        "the public bind proceeded: {parsed}"
    );
}

/// A public bind warns in normal mode too: a credential store is configured, so
/// the anonymous-bind refusal does not fire, and the warning still does. The
/// store is a per-test scratch file that removes itself with the guard.
#[test]
fn a_public_bind_warns_with_a_credential_store() {
    let credentials = NodeDir::new("bind-credentials");
    let store = write_credential_store(&credentials);
    let mut child = spawn_serve(
        &credentials,
        &[
            "--bind",
            "0.0.0.0:0",
            "--token-file",
            store.to_str().expect("utf-8 scratch path"),
            "--ttl-secs",
            "30",
            "--db",
            "sqlite::memory:",
        ],
    );
    let stderr = stderr_until(&mut child, WARNING_PREFIX);
    assert!(
        stderr.contains(WARNING_PREFIX),
        "no warning on a public bind with a credential store: {stderr:?}"
    );
    let _ = kill_and_collect_stdout(child);
}

/// A loopback bind prints no warning: the same start, silent stderr.
#[test]
fn a_loopback_bind_stays_silent() {
    let scratch = NodeDir::new("bind-loopback");
    let mut child = spawn_serve(
        &scratch,
        &[
            "--bind",
            "127.0.0.1:0",
            "--dev",
            "--ttl-secs",
            "30",
            "--output",
            "text",
            "--db",
            "sqlite::memory:",
        ],
    );
    let listening = stdout_listening(&mut child);
    assert!(listening, "the loopback bind must proceed");
    let stderr = kill_and_collect_stderr(child);
    let text = String::from_utf8_lossy(&stderr);
    assert!(
        !text.contains(WARNING_PREFIX),
        "loopback must not warn: {text:?}"
    );
}

/// The anonymous non-loopback bind outside dev mode is still refused, and the
/// refusal is on stderr, so the warning never replaces the refusal.
#[test]
fn an_anonymous_public_bind_outside_dev_mode_is_still_refused() {
    let scratch = NodeDir::new("bind-anonymous");
    let mut child = spawn_serve(
        &scratch,
        &[
            "--bind",
            "0.0.0.0:0",
            "--ttl-secs",
            "30",
            "--db",
            "sqlite::memory:",
        ],
    );
    let stderr = stderr_until(&mut child, "refusing anonymous bind");
    assert!(
        stderr.contains("refusing anonymous bind"),
        "the refusal must stay: {stderr:?}"
    );
    let stdout = kill_and_collect_stdout(child);
    assert!(
        stdout.is_empty(),
        "a refused bind must not answer: {:?}",
        String::from_utf8_lossy(&stdout)
    );
}
