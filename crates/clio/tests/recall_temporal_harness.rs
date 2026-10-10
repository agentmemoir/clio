// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `recall --query-anchor` binding of the temporal
//! retrieval arm against the real `clio` binary: it seeds one memory with
//! `remember`, reads the memory's own `created_at` from `inspect`, and then
//! checks that a relative phrase resolves inside that window with an explicit
//! anchor, that the same phrase without an anchor contributes nothing, that a
//! window that excludes the memory returns no hits, and that a malformed anchor
//! fails closed.
//!
//! ## Owns
//! - End-to-end temporal-arm assertions over spawned `clio` processes
//!
//! ## Does not own
//! - The flag-to-argument mapping (`cli_read_temporal_tests`)
//! - Expression parsing or window resolution (`clio-retrieve` `temporal`)
//!
//! ## Boundary
//!
//! Tests here break when the real binary stops honoring the anchor; they never
//! change it.
//!
//! Keep this module focused on the spawned-binary temporal arm.

mod common;

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

/// Calls served so far in this test binary, so `temp_dir` never returns a
/// directory another scenario already created.
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// The seeded memory's gist; its words never appear in the query text, so only
/// the temporal arm can return it.
const MARKER: &str = "temporal harness marker zeta";

/// Unique temp directory for one scenario.
fn temp_dir(tag: &str) -> PathBuf {
    // The counter is what makes this distinct: the process id is constant
    // inside one test binary, so clock-plus-pid alone repeats whenever two
    // calls land in the same tick, and two scenarios then share one
    // directory and read each other's database.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "clio-recall-temporal-{tag}-{}-{nanos}-{sequence}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

/// The `clio` binary with a cleared environment plus the coverage pattern.
fn clio() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_clio"));
    cmd.env_clear();
    // Forward the coverage profile pattern so instrumented children write into
    // the gitignored target directory instead of the source tree.
    if let Some(pattern) = std::env::var_os("LLVM_PROFILE_FILE") {
        cmd.env("LLVM_PROFILE_FILE", pattern);
    }
    common::seed_key(&mut cmd);
    cmd
}

/// Run one command against the scenario database with piped streams.
fn run(db: &Path, args: &[&str]) -> Output {
    clio()
        .args(args)
        .arg("--db")
        .arg(db)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn clio")
}

fn stdout_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Parse one command's stdout as JSON, with the raw streams on failure.
fn json_stdout(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout is not JSON: {e}\nstdout: {}\nstderr: {}",
            stdout_of(output),
            stderr_of(output)
        )
    })
}

/// Parse the one error envelope the CLI writes to stderr.
fn json_stderr(output: &Output) -> Value {
    serde_json::from_str(stderr_of(output).trim()).unwrap_or_else(|e| {
        panic!(
            "stderr is not JSON: {e}\nstdout: {}\nstderr: {}",
            stdout_of(output),
            stderr_of(output)
        )
    })
}

/// Seed one memory through the real binary.
fn seed_scenario(tag: &str) -> PathBuf {
    let dir = temp_dir(tag);
    let db = dir.join("clio.db");
    let output = run(
        &db,
        &[
            "remember",
            MARKER,
            "--category",
            "tool_config",
            "--output",
            "json",
        ],
    );
    assert!(
        output.status.success(),
        "seed failed: {}",
        stderr_of(&output)
    );
    dir
}

/// The seeded memory's `(item_id, created_at)` from the metadata-only listing.
///
/// The item's own creation time is a deterministic anchor for its own "today":
/// no wall-clock formatting is needed in this test.
fn seeded_identity(db: &Path) -> (String, String) {
    let output = run(db, &["inspect", "--output", "json"]);
    assert!(
        output.status.success(),
        "inspect failed: {}",
        stderr_of(&output)
    );
    let payload = json_stdout(&output);
    let rows = payload["items"].as_array().expect("items array");
    assert_eq!(rows.len(), 1, "one seeded item: {payload}");
    let item_id = rows[0]["item_id"].as_str().expect("item_id").to_owned();
    let created_at = rows[0]["created_at"]
        .as_str()
        .expect("created_at")
        .to_owned();
    (item_id, created_at)
}

/// A successful `recall` JSON payload, with the streams on failure.
fn recall_json(db: &Path, args: &[&str]) -> Value {
    let output = run(db, args);
    assert!(
        output.status.success(),
        "{args:?} failed: {}",
        stderr_of(&output)
    );
    let payload = json_stdout(&output);
    assert_eq!(payload["ok"], Value::Bool(true), "{args:?}: {payload}");
    payload
}

fn hits(payload: &Value) -> &[Value] {
    payload["hits"]
        .as_array()
        .unwrap_or_else(|| panic!("hits array missing: {payload}"))
}

/// An explicit anchor on the memory's own creation day returns the memory with
/// its temporal rank; the same phrase without the anchor contributes nothing.
#[test]
fn recall_resolves_a_relative_phrase_only_with_an_anchor() {
    let dir = seed_scenario("anchor");
    let db = dir.join("clio.db");
    let (item_id, created_at) = seeded_identity(&db);

    let anchored = recall_json(
        &db,
        &[
            "recall",
            "what did I do today",
            "--query-anchor",
            &created_at,
            "--output",
            "json",
        ],
    );
    let anchored_hits = hits(&anchored);
    assert_eq!(anchored_hits.len(), 1, "the window hits: {anchored}");
    assert_eq!(anchored_hits[0]["item_id"], Value::from(item_id.as_str()));
    assert_eq!(anchored_hits[0]["temporal_rank"], Value::from(1));
    assert_eq!(anchored["temporal_candidates"], Value::from(1));

    let unanchored = recall_json(&db, &["recall", "what did I do today", "--output", "json"]);
    assert!(
        hits(&unanchored).is_empty(),
        "no anchor means no arm: {unanchored}"
    );
    assert!(
        unanchored.get("temporal_candidates").is_none(),
        "no arm means no counter: {unanchored}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A window that excludes the memory returns an empty page from a running arm,
/// and the counter still reports the arm ran with nothing kept.
#[test]
fn recall_window_that_excludes_the_memory_is_empty() {
    let dir = seed_scenario("excluded");
    let db = dir.join("clio.db");
    let (_, created_at) = seeded_identity(&db);

    let payload = recall_json(
        &db,
        &[
            "recall",
            "what did I do yesterday",
            "--query-anchor",
            &created_at,
            "--output",
            "json",
        ],
    );
    assert!(
        hits(&payload).is_empty(),
        "yesterday excludes today: {payload}"
    );
    assert_eq!(payload["temporal_candidates"], Value::from(0));

    let _ = std::fs::remove_dir_all(&dir);
}

/// A malformed anchor fails closed with the documented tool error, and the
/// phrase never falls back to server time.
#[test]
fn recall_with_a_malformed_anchor_fails_closed() {
    let dir = seed_scenario("bad-anchor");
    let db = dir.join("clio.db");

    let output = run(
        &db,
        &[
            "recall",
            "what did I do today",
            "--query-anchor",
            "2026-09-26",
            "--output",
            "json",
        ],
    );
    assert!(!output.status.success(), "a bare date must fail");
    let envelope = json_stderr(&output);
    assert_eq!(
        envelope["code"],
        Value::from("invalid_argument"),
        "{envelope}"
    );
    assert!(
        envelope["message"]
            .as_str()
            .expect("message")
            .contains("--query-anchor"),
        "the error names the flag the caller typed: {envelope}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
