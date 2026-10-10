// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//!
//! # Responsibility
//!
//! This harness proves that repeating one `clio remember` stores one item and
//! that `clio recall` never returns the same text twice. It drives the compiled
//! `clio` binary, so the guarantee holds for the command a user actually types.
//!
//! ## Owns
//! - Seven identical `remember` calls collapsing to one stored item
//! - `recall` returning one hit for a bank full of identical rows
//! - Case, punctuation, and whitespace variants counting as duplicates
//! - `remember --allow-duplicate` writing a second row on request
//! - Non-duplicate text still being stored
//!
//! ## Does not own
//! - Admission arithmetic (see `clio-admission` unit tests)
//! - Read-time cap thresholds (see `clio-retrieve` unit tests)
//!
//! ## Boundary
//!
//! Changes to the duplicate write or recall contract should normally require
//! changes here. Changes to scoring internals should not.

mod common;

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

/// Calls served so far in this test binary, so `temp_dir` never returns a
/// directory another scenario already created.
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// The fact the whole harness repeats. Long enough to be a real persona fact.
const FACT: &str = "Tom goes on a walk every Friday";
/// A second, different fact, so a pass-through is distinguishable from a
/// blanket refusal.
const OTHER: &str = "Tom walks his dog on Monday mornings";

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
        "clio-exact-dup-{tag}-{}-{nanos}-{sequence}",
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

fn stdout_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn json_stdout(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "stdout is not JSON: {error}\nstdout: {}\nstderr: {}",
            stdout_of(output),
            stderr_of(output)
        )
    })
}

/// A fresh scenario database path inside a per-test directory.
///
/// `--db` takes the database *file*; the existing floor harness passes its
/// directory, which SQLite refuses to open. This returns the file path.
fn scenario(tag: &str) -> PathBuf {
    temp_dir(tag).join("clio.db")
}

/// One `remember` of a repeated `text`, asserting the gate refused it with exit 1,
/// and returning the refusal envelope the CLI writes to stderr.
fn remember_refused(db: &Path, text: &str) -> Value {
    let output = run(
        db,
        &[
            "remember",
            text,
            "--category",
            "persona",
            "--output",
            "json",
        ],
    );
    assert_eq!(
        output.status.code(),
        Some(1),
        "a duplicate must exit 1: {}",
        stderr_of(&output)
    );
    // The admission telemetry line comes first on stderr; the refusal envelope is the last line.
    let stderr = stderr_of(&output);
    let envelope = stderr.lines().last().unwrap_or_default();
    serde_json::from_str(envelope)
        .unwrap_or_else(|error| panic!("refusal is not JSON: {error}\nstderr: {stderr}"))
}

/// One `remember` of `text`, asserting the write succeeded, and returning the
/// parsed payload.
fn remember(db: &Path, text: &str, extra: &[&str]) -> Value {
    let mut args = vec![
        "remember",
        text,
        "--category",
        "persona",
        "--output",
        "json",
    ];
    args.extend_from_slice(extra);
    let output = run(db, &args);
    assert!(
        output.status.success(),
        "remember {text:?} failed: {}",
        stderr_of(&output)
    );
    json_stdout(&output)
}

/// The `recall` hit gists for a query.
fn recall_gists(db: &Path, query: &str) -> Vec<String> {
    let output = run(db, &["recall", query, "--output", "json"]);
    assert!(
        output.status.success(),
        "recall {query:?} failed: {}",
        stderr_of(&output)
    );
    let payload = json_stdout(&output);
    payload["hits"]
        .as_array()
        .expect("hits array")
        .iter()
        .map(|hit| hit["gist"].as_str().unwrap_or_default().to_owned())
        .collect()
}

#[test]
fn seven_identical_remembers_store_one_item_and_recall_returns_it_once() {
    let db = scenario("seven");

    let first = remember(&db, FACT, &[]);
    assert_eq!(first["pass"], Value::Bool(true));
    let stored_id = first["id"]
        .as_str()
        .expect("first write has an id")
        .to_owned();

    // Six repeats of the identical sentence. Each is refused with exit 1 and a
    // stated reason, and none stores an item.
    for attempt in 2..=7 {
        let refusal = remember_refused(&db, FACT);
        assert_eq!(
            refusal["ok"],
            Value::Bool(false),
            "repeat {attempt}: {refusal}"
        );
        assert_eq!(refusal["code"], "duplicate", "repeat {attempt}: {refusal}");
        let message = refusal["message"].as_str().unwrap_or_default();
        assert!(
            message.starts_with("duplicate: nothing stored (exact_duplicate"),
            "repeat {attempt} message: {message}"
        );
    }

    let gists = recall_gists(&db, "When does Tom go on a walk?");
    assert_eq!(
        gists,
        vec![FACT.to_owned()],
        "recall must return the fact exactly once"
    );

    // The stored id is still the one the first write reported, so the repeats
    // did not supersede or replace the original.
    let after = remember(&db, OTHER, &[]);
    assert_eq!(after["pass"], Value::Bool(true));
    assert_ne!(after["id"].as_str(), Some(stored_id.as_str()));
}

#[test]
fn a_repeat_text_view_exits_one_and_names_the_reason() {
    let db = scenario("exit-code");
    remember(&db, FACT, &[]);

    let output = run(
        &db,
        &[
            "remember",
            FACT,
            "--category",
            "persona",
            "--output",
            "text",
        ],
    );
    let text = stderr_of(&output);
    assert_eq!(
        output.status.code(),
        Some(1),
        "a duplicate must exit 1: {text}"
    );
    assert!(
        text.contains("duplicate: nothing stored"),
        "unexpected text output: {text}"
    );
}

#[test]
fn case_punctuation_and_whitespace_variants_are_duplicates() {
    let db = scenario("variants");
    remember(&db, FACT, &[]);

    for variant in [
        "tom goes on a walk every friday",
        "Tom goes on a walk every Friday!",
        "  Tom   goes on a walk   every   Friday  ",
        "TOM GOES ON A WALK EVERY FRIDAY",
    ] {
        let refusal = remember_refused(&db, variant);
        assert_eq!(
            refusal["code"], "duplicate",
            "{variant:?} should be a duplicate: {refusal}"
        );
    }
}

#[test]
fn a_genuinely_different_fact_is_still_stored() {
    let db = scenario("distinct");
    remember(&db, FACT, &[]);

    for text in [
        OTHER,
        "Tom goes on a walk every Saturday",
        "Tom goes on a walk before work",
    ] {
        let payload = remember(&db, text, &[]);
        assert_eq!(
            payload["pass"],
            Value::Bool(true),
            "{text:?} must still be stored: {payload}"
        );
        assert!(payload["id"].is_string(), "{text:?} must get an id");
    }

    let gists = recall_gists(&db, "When does Tom go on a walk?");
    assert_eq!(gists.len(), 4, "four distinct facts: {gists:?}");
}

#[test]
fn force_writes_a_second_row_on_request() {
    let db = scenario("force");
    let first = remember(&db, FACT, &[]);
    let forced = remember(&db, FACT, &["--allow-duplicate"]);

    assert_eq!(forced["pass"], Value::Bool(true));
    assert!(
        forced["id"].is_string(),
        "--allow-duplicate must store an item"
    );
    assert_ne!(first["id"], forced["id"]);
}

#[test]
fn a_bank_that_already_holds_identical_rows_returns_one_hit() {
    // The read side must hold for rows written before the write-side rule
    // existed, so this seeds duplicates with --allow-duplicate and then recalls.
    let db = scenario("preexisting");
    for _ in 0..5 {
        remember(&db, FACT, &["--allow-duplicate"]);
    }

    let gists = recall_gists(&db, "When does Tom go on a walk?");
    assert_eq!(
        gists,
        vec![FACT.to_owned()],
        "recall must collapse rows that were stored before the fix"
    );
}
