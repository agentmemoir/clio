// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module proves the live `clio correct --confirm` path across separate
//! CLI processes, on SQLite and on Postgres: a correction must mint a fresh
//! revision id each time, so a second correction never collides with the first
//! correction's stored id.
//!
//! ## Owns
//! - The cross-process remember → correct → correct → get scenario on both backends
//! - The per-run disposable SQLite store, the unique Postgres bank, and the CLI runner
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

use clio::store::lock_pg_test;
use common::seed_key;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

/// Local Compose Postgres, the same URL the Makefile exports for parity.
const COMPOSE_PG: &str = "postgres://clio:clio@127.0.0.1:34310/clio";

/// Per-binary call counter, so two tests never share a scenario name.
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// A name unique to one call: process id plus a monotonic sequence.
fn unique(tag: &str) -> String {
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("correct-{tag}-{}-{sequence}", std::process::id())
}

/// The Postgres URL to drive: the parent `CLIO_DATABASE_URL` (the CI service),
/// falling back to local Compose.
fn pg_url() -> String {
    std::env::var("CLIO_DATABASE_URL").unwrap_or_else(|_| COMPOSE_PG.to_owned())
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(unique(tag));
    fs::create_dir_all(&dir).expect("temp dir");
    dir
}

/// The `clio` binary with a cleared environment, the seeded key file, and the
/// scenario's own KMS file so its DEKs never touch the tree.
fn clio(kms: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_clio"));
    cmd.env_clear();
    if let Some(pattern) = std::env::var_os("LLVM_PROFILE_FILE") {
        cmd.env("LLVM_PROFILE_FILE", pattern);
    }
    cmd.env("CLIO_KMS_FILE", kms);
    seed_key(&mut cmd);
    cmd
}

/// Run one command against the scenario database with piped streams. `db` is a
/// SQLite path or a Postgres URL.
fn run(kms: &Path, db: &str, args: &[&str]) -> Output {
    clio(kms)
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

/// The shared scenario: `remember`, then two live corrections in separate
/// process invocations, then read the newest revision. `bank_args` is empty for
/// SQLite (the default bank) and `["--bank", <unique>]` for Postgres, so a
/// SQLite scenario does not fill a shared Postgres bank and vice versa.
fn two_live_corrections(kms: &Path, db: &str, bank_args: &[&str]) {
    let remember: Vec<&str> = [
        "remember",
        "printer floor three",
        "--category",
        "task_spec",
        "--output",
        "json",
    ]
    .into_iter()
    .chain(bank_args.iter().copied())
    .collect();
    let stored = run(kms, db, &remember);
    assert!(stored.status.success(), "{}", stderr_of(&stored));
    let prior = json_stdout(&stored)["id"]
        .as_str()
        .expect("the stored item's id")
        .to_owned();

    // First live correction: a fresh process mints a new revision.
    let first: Vec<&str> = [
        "correct",
        &prior,
        "printer floor four",
        "--reason",
        "moved",
        "--confirm",
        "--output",
        "json",
    ]
    .into_iter()
    .chain(bank_args.iter().copied())
    .collect();
    let first = run(kms, db, &first);
    assert!(first.status.success(), "{}", stderr_of(&first));
    let revision = json_stdout(&first)["correction"]["new_item_id"]
        .as_str()
        .expect("the first revision id")
        .to_owned();
    assert_ne!(revision, prior, "the correction must store a new id");

    // Second live correction: before the fix the one-shot process restarted its
    // id counter and minted the same revision id again, so this insert failed on
    // the items primary key.
    let second: Vec<&str> = [
        "correct",
        &revision,
        "printer floor five",
        "--reason",
        "moved again",
        "--confirm",
        "--output",
        "json",
    ]
    .into_iter()
    .chain(bank_args.iter().copied())
    .collect();
    let second = run(kms, db, &second);
    assert!(second.status.success(), "{}", stderr_of(&second));
    let latest = json_stdout(&second)["correction"]["new_item_id"]
        .as_str()
        .expect("the second revision id")
        .to_owned();
    assert_ne!(latest, revision, "each correction must store a new id");
    assert_ne!(latest, prior, "each correction must store a new id");

    // The newest revision reads back the corrected value.
    let read: Vec<&str> = ["get", &latest, "--output", "json"]
        .into_iter()
        .chain(bank_args.iter().copied())
        .collect();
    let read = run(kms, db, &read);
    assert!(read.status.success(), "{}", stderr_of(&read));
    assert_eq!(
        json_stdout(&read)["gist"],
        Value::from("printer floor five")
    );
}

/// A live correction that runs after an earlier one, in a fresh process, must
/// not reuse the first correction's revision id.
#[test]
fn live_correct_mints_a_fresh_revision_id_each_process() {
    let dir = temp_dir("fresh-id");
    let db = dir.join("clio.db");
    let kms = dir.join("clio.kms.json");
    two_live_corrections(&kms, db.to_str().expect("scenario path"), &[]);
    fs::remove_dir_all(&dir).expect("remove the scenario directory");
}

/// The same scenario on Postgres, where the issue was reported. Fails closed
/// when no server answers: the gate panics unless `CLIO_SKIP_PG_TESTS` opted
/// out, in which case it prints the credential-safe note and returns.
#[test]
fn live_correct_mints_a_fresh_revision_id_each_process_on_postgres() {
    let url = pg_url();
    if !clio::store::pg_test_target::postgres_reachable(&url) {
        clio::store::pg_skip::pg_required_or_skip(
            "the Postgres correction scenario",
            &format!(
                "no Postgres at {}",
                clio::store::pg_test_target::redacted_endpoint(&url)
            ),
        );
        return;
    }
    let _guard = lock_pg_test();
    let kms_dir = temp_dir("pg-kms");
    let kms = kms_dir.join("clio.kms.json");
    let bank = unique("bank");
    two_live_corrections(&kms, &url, &["--bank", &bank]);
    fs::remove_dir_all(&kms_dir).expect("remove the scenario directory");
}
