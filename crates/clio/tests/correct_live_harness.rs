// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module proves the live `clio correct --confirm` path across separate
//! CLI processes: a correction must mint a fresh revision id each time, so a
//! second correction never collides with the first correction's stored id.
//!
//! ## Owns
//! - The cross-process remember → correct → correct → get scenario
//! - The per-run disposable sqlite store and its CLI runner
//!
//! ## Does not own
//! - The correction engine semantics (`clio-compliance`, `clio-store`)
//! - The id minting (`clio-mcp::runtime_context`) or the help surfaces
//!
//! ## Boundary
//!
//! Tests here break when a caller-omitted item id repeats across one-shot CLI
//! processes; they never change the correction or the id policy.
//!
//! Keep this module focused on the live-correction id contract.

mod common;

use common::seed_key;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

/// Per-binary call counter, so two tests never share a scenario directory.
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn temp_dir(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "clio-correct-live-{tag}-{}-{nanos}-{sequence}",
        std::process::id()
    ));
    fs::create_dir_all(&dir).expect("temp dir");
    dir
}

/// The `clio` binary with a cleared environment plus the seeded key file.
fn clio() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_clio"));
    cmd.env_clear();
    if let Some(pattern) = std::env::var_os("LLVM_PROFILE_FILE") {
        cmd.env("LLVM_PROFILE_FILE", pattern);
    }
    seed_key(&mut cmd);
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

fn stderr_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Parse one command's stdout as JSON, with the raw stderr on failure.
fn json_stdout(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "stdout is not JSON: {error}\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            stderr_of(output)
        )
    })
}

/// A live correction that runs after an earlier one, in a fresh process, must
/// not reuse the first correction's revision id.
#[test]
fn live_correct_mints_a_fresh_revision_id_each_process() {
    let dir = temp_dir("fresh-id");
    let db = dir.join("clio.db");

    // Process 1: the remembered item is stored under the CLI's own id.
    let stored = run(
        &db,
        &[
            "remember",
            "printer floor three",
            "--category",
            "task_spec",
            "--output",
            "json",
        ],
    );
    assert!(stored.status.success(), "{}", stderr_of(&stored));
    let prior = json_stdout(&stored)["id"]
        .as_str()
        .expect("the stored item's id")
        .to_owned();

    // Process 2: the first live correction mints a new revision.
    let first = run(
        &db,
        &[
            "correct",
            &prior,
            "printer floor four",
            "--reason",
            "moved",
            "--confirm",
            "--output",
            "json",
        ],
    );
    assert!(first.status.success(), "{}", stderr_of(&first));
    let revision = json_stdout(&first)["correction"]["new_item_id"]
        .as_str()
        .expect("the first revision id")
        .to_owned();
    assert_ne!(revision, prior, "the correction must store a new id");

    // Process 3: a second live correction of that revision must succeed. Before
    // the fix the one-shot process restarted its id counter and minted the same
    // revision id again, so this insert failed on the items primary key.
    let second = run(
        &db,
        &[
            "correct",
            &revision,
            "printer floor five",
            "--reason",
            "moved again",
            "--confirm",
            "--output",
            "json",
        ],
    );
    assert!(second.status.success(), "{}", stderr_of(&second));
    let latest = json_stdout(&second)["correction"]["new_item_id"]
        .as_str()
        .expect("the second revision id")
        .to_owned();
    assert_ne!(latest, revision, "each correction must store a new id");
    assert_ne!(latest, prior, "each correction must store a new id");

    // Process 4: the newest revision reads back the corrected value.
    let read = run(&db, &["get", &latest, "--output", "json"]);
    assert!(read.status.success(), "{}", stderr_of(&read));
    assert_eq!(
        json_stdout(&read)["gist"],
        Value::from("printer floor five")
    );

    fs::remove_dir_all(&dir).expect("remove the scenario directory");
}
