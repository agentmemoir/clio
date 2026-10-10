// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `-o` alias for `--output` against the real `clio`
//! binary: recall JSON/text selection, the before-verb form, and fail-closed
//! errors for a missing value and an unknown short token.
//!
//! ## Owns
//! - End-to-end output-mode assertions over spawned `clio` processes
//!
//! ## Does not own
//! - In-process parser/output unit tests (`cli_args_tests`, `cli_output_tests`)
//! - The CLI implementation under test
//!
//! ## Boundary
//!
//! Tests here break when the real binary's output-mode contract changes; they
//! never change it.
//!
//! Keep this module focused on the `-o`/`--output` process contract.

mod common;

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

/// Calls served so far in this test binary, so `temp_dir` never returns a
/// directory another scenario already created.
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

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
        "clio-output-alias-{tag}-{}-{nanos}-{sequence}",
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

/// Parse one command's stdout as JSON, with the raw streams on failure.
fn json_stdout(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout is not JSON: {e}\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn stderr_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn short_output_alias_matches_the_long_flag_end_to_end() {
    let dir = temp_dir("recall");
    let db = dir.join("clio.db");

    // Seed one item through the real binary; a CLI write drains the index, so
    // the next process can recall it.
    let seed = run(
        &db,
        &[
            "remember",
            "output alias harness fact",
            "--category",
            "persona",
            "--output",
            "json",
        ],
    );
    assert!(seed.status.success(), "seed failed: {}", stderr_of(&seed));

    // `-o json` after the verb and the joined `-o=json` both emit JSON.
    for args in [
        vec!["recall", "alias", "-o", "json"],
        vec!["recall", "alias", "-o=json"],
    ] {
        let output = run(&db, &args);
        assert!(output.status.success(), "{args:?}: {}", stderr_of(&output));
        let payload = json_stdout(&output);
        assert_eq!(payload["ok"], Value::Bool(true), "{args:?}: {payload}");
        assert!(
            !payload["hits"].as_array().expect("hits").is_empty(),
            "{args:?}: {payload}"
        );
    }

    // `-o text` while piped still prints human text.
    let output = run(&db, &["recall", "alias", "-o", "text"]);
    assert!(output.status.success(), "{}", stderr_of(&output));
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(text.contains("Results for: alias"), "{text}");

    // `-o json` before the verb behaves like the after-verb form.
    let output = clio()
        .args(["-o", "json", "--db"])
        .arg(&db)
        .args(["recall", "alias"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn clio");
    assert!(output.status.success(), "{}", stderr_of(&output));
    assert_eq!(json_stdout(&output)["ok"], Value::Bool(true));

    // `-o` without a value and an unknown short token still fail closed.
    let output = run(&db, &["recall", "alias", "-o"]);
    assert_eq!(output.status.code(), Some(2), "{}", stderr_of(&output));
    assert!(
        stderr_of(&output).contains("requires a value"),
        "{}",
        stderr_of(&output)
    );

    let output = run(&db, &["recall", "alias", "-x"]);
    assert_eq!(output.status.code(), Some(2), "{}", stderr_of(&output));
    assert!(
        stderr_of(&output).contains("unknown flag `-x`"),
        "{}",
        stderr_of(&output)
    );

    let _ = std::fs::remove_dir_all(&dir);
}
