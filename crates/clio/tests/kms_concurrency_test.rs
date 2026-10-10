// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the local dev-KMS key store under real multi-process
//! contention: concurrent `clio remember` processes share one store and every
//! item must stay decryptable.
//!
//! ## Owns
//! - Concurrent first-time store creation and concurrent key writes
//! - Decryption of every concurrently written item in later processes
//!
//! ## Does not own
//! - Store internals (`clio-store` unit tests)
//! - Single-process CLI behavior (see the in-crate write tests)
//!
//! ## Boundary
//!
//! Tests here break when the key store stops being safe for concurrent
//! processes; they never change the implementation.
//!
//! Keep this module focused on multi-process key-store behavior.

mod common;

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
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
        "am-kms-conc-{tag}-{}-{nanos}-{sequence}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

/// The `clio` binary with a cleared environment plus the supplied overrides.
fn clio_cmd(envs: &[(&str, &str)]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_clio"));
    cmd.env_clear();
    // Forward the coverage profile pattern so instrumented children write into
    // the gitignored target directory instead of the source tree.
    if let Some(pattern) = std::env::var_os("LLVM_PROFILE_FILE") {
        cmd.env("LLVM_PROFILE_FILE", pattern);
    }
    for (key, value) in envs {
        cmd.env(key, value);
    }
    common::seed_key(&mut cmd);
    cmd
}

fn run_remember(db: &Path, text: &str) -> std::process::Output {
    clio_cmd(&[])
        .arg("remember")
        .arg(text)
        .args(["--category", "persona", "--db"])
        .arg(db)
        .args(["--output", "json"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn clio remember")
}

fn run_get(db: &Path, id: &str) -> std::process::Output {
    clio_cmd(&[])
        .arg("get")
        .arg(id)
        .args(["--db"])
        .arg(db)
        .args(["--output", "json"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn clio get")
}

fn json_stdout(output: &std::process::Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout is not JSON: {e}\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

#[test]
fn concurrent_remember_processes_share_one_store_and_lose_nothing() {
    let dir = temp_dir("remember");
    let db = dir.join("clio.db");
    let writers = 6;

    // Spawn every writer before joining any, so creation and key writes overlap.
    let mut handles = Vec::with_capacity(writers);
    for i in 0..writers {
        let db = db.clone();
        handles.push(std::thread::spawn(move || {
            run_remember(&db, &format!("concurrent fact {i}"))
        }));
    }
    let mut outputs = Vec::with_capacity(writers);
    for handle in handles {
        outputs.push(handle.join().expect("writer thread"));
    }

    for (i, output) in outputs.iter().enumerate() {
        assert!(
            output.status.success(),
            "writer {i} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    // Every concurrently written item must decrypt in a later process.
    let ids: Vec<String> = outputs
        .iter()
        .map(|output| json_stdout(output)["id"].as_str().unwrap().to_owned())
        .collect();
    for (i, id) in ids.iter().enumerate() {
        let output = run_get(&db, id);
        assert!(
            output.status.success(),
            "get {id} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let payload = json_stdout(&output);
        assert_eq!(payload["ok"], Value::Bool(true), "get {id}: {payload}");
        assert_eq!(payload["gist"], format!("concurrent fact {i}"));
    }

    // No creation or migration leftovers beside the store.
    let leftovers: Vec<_> = std::fs::read_dir(&dir)
        .expect("read dir")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains(".create"))
        .collect();
    assert!(leftovers.is_empty(), "leftover files: {leftovers:?}");

    let _ = std::fs::remove_dir_all(&dir);
}
