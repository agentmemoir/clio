// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests that the real `clio` binary names the floor that emptied a
//! `recall` page with the flag the caller typed, for each floor a lexical store
//! can bind (`--min-keyword` and `--min-final`). `--min-semantic` and
//! `--min-reranker` need a dense or rerank arm and are covered by the retrieval
//! domain tests instead.
//!
//! ## Owns
//! - Spawned-binary assertions that the empty-page warning names the typed flag
//!
//! ## Does not own
//! - The floor binding rule (`clio-retrieve` `floors` / `finalize`)
//! - The `--min-final` end-to-end case in `recall_floors_harness`
//!
//! ## Boundary
//!
//! Tests here break when the real binary stops naming the flag; they never
//! change it.

mod common;

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

/// Calls served so far in this test binary, so two scenarios never share a dir.
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Unique temp directory for one scenario.
fn temp_dir(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "clio-recall-floor-name-{tag}-{}-{nanos}-{sequence}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

/// The `clio` binary with a cleared environment plus the coverage pattern.
fn clio() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_clio"));
    cmd.env_clear();
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

fn stderr_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Seed one memory so a lexical recall has a hit for a floor to remove.
fn seed(db: &Path) {
    let output = run(
        db,
        &[
            "remember",
            "terse answers beat verbose replies",
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
}

/// A successful recall payload, with the streams on failure.
fn recall_json(db: &Path, args: &[&str]) -> Value {
    let output = run(db, args);
    assert!(output.status.success(), "{args:?}: {}", stderr_of(&output));
    serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|e| panic!("stdout is not JSON: {e}\nstderr: {}", stderr_of(&output)))
}

/// A floor that empties the page must name the flag the caller typed.
fn assert_names_flag(db: &Path, floor: &str, value: &str) {
    let out = recall_json(db, &["recall", "terse", floor, value, "--output", "json"]);
    assert!(
        out["hits"].as_array().is_some_and(Vec::is_empty),
        "{floor} {value} must empty the page: {out}"
    );
    assert!(
        out["warnings"].as_array().is_some_and(|warnings| warnings
            .iter()
            .any(|warning| warning.as_str().is_some_and(|text| text.contains(floor)))),
        "the empty page names {floor}: {out}"
    );
}

#[test]
fn a_keyword_floor_names_the_flag() {
    let dir = temp_dir("keyword");
    let db = dir.join("clio.db");
    seed(&db);
    assert_names_flag(&db, "--min-keyword", "1000000");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_final_floor_names_the_flag() {
    let dir = temp_dir("final");
    let db = dir.join("clio.db");
    seed(&db);
    assert_names_flag(&db, "--min-final", "1000");
    let _ = std::fs::remove_dir_all(&dir);
}
