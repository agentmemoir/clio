// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `recall` score-floor flags and the two
//! entity-inclusion controls against the real `clio` binary: it spawns
//! `remember` to seed a database, then spawns `recall` to check that a floor
//! can empty a page without failing, that the default call is unchanged and
//! carries no summary, that `--no-entities` empties every hit's `entities`, that
//! `--entity-summary` adds the top-level object, and that a bad flag value
//! fails with the same usage-error shape as a bad `--limit`.
//!
//! ## Owns
//! - End-to-end floor and entity-control assertions over spawned `clio`
//!   processes
//!
//! ## Does not own
//! - The flag-to-argument mapping (`cli_read_floors_tests`)
//! - The floor and entity rules themselves (`clio-retrieve`)
//! - The unchanged default recall payload (`recall_text_scores_harness`)
//!
//! ## Boundary
//!
//! Tests here break when the real binary stops honoring the flags; they never
//! change it.
//!
//! Keep this module focused on the spawned-binary floor and entity controls.

mod common;

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

/// Calls served so far in this test binary, so `temp_dir` never returns a
/// directory another scenario already created.
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// A floor no fused score in this deployment can reach, so every candidate is
/// filtered out. `final` is a weighted rank-fusion sum, so a large value is a
/// legal request that returns nothing rather than an error.
const UNREACHABLE_FINAL_FLOOR: &str = "1000";

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
        "clio-recall-floors-{tag}-{}-{nanos}-{sequence}",
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

fn stderr_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn stdout_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
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

/// The one error envelope the CLI writes to stderr, with the streams on failure.
fn json_stderr(output: &Output) -> Value {
    serde_json::from_str(stderr_of(output).trim()).unwrap_or_else(|e| {
        panic!(
            "stderr is not JSON: {e}\nstdout: {}\nstderr: {}",
            stdout_of(output),
            stderr_of(output)
        )
    })
}

/// Seed two memories through the real binary and return the scenario directory.
///
/// A CLI write drains the index, so the next process can recall what was
/// stored. The query term `terse` appears in the first memory only, so the
/// floored and unfloored runs below differ by the floor and nothing else.
fn seed_scenario(tag: &str) -> PathBuf {
    let dir = temp_dir(tag);
    let db = dir.join("clio.db");
    for text in [
        "terse answers beat verbose replies",
        "plain notes about the build",
    ] {
        let output = run(
            &db,
            &[
                "remember",
                text,
                "--category",
                "tool_config",
                "--output",
                "json",
            ],
        );
        assert!(
            output.status.success(),
            "seed {text:?} failed: {}",
            stderr_of(&output)
        );
        assert_eq!(
            json_stdout(&output)["ok"],
            Value::Bool(true),
            "seed {text:?}"
        );
    }
    dir
}

/// A successful `recall` JSON payload, with the streams on failure.
fn recall_json(db: &Path, args: &[&str]) -> Value {
    let output = run(db, args);
    assert!(
        output.status.success(),
        "{args:?} failed: {}",
        stderr_of(&output)
    );
    assert!(
        stderr_of(&output).is_empty(),
        "{args:?} stderr: {}",
        stderr_of(&output)
    );
    let payload = json_stdout(&output);
    assert_eq!(payload["ok"], Value::Bool(true), "{args:?}: {payload}");
    payload
}

/// The returned hits of a payload, which must always be an array.
fn hits(payload: &Value) -> &[Value] {
    payload["hits"]
        .as_array()
        .unwrap_or_else(|| panic!("hits array missing: {payload}"))
}

/// A floor the page cannot clear is a normal success with zero hits, and the
/// same query without the floor still returns hits. The floor only removes
/// candidates, so the counters above the page still report what was fused.
#[test]
fn recall_with_an_unreachable_final_floor_returns_an_empty_page() {
    let dir = seed_scenario("final-floor");
    let db = dir.join("clio.db");
    let query = ["recall", "terse", "--output", "json"];

    let unfloored = recall_json(&db, &query);
    assert!(
        !hits(&unfloored).is_empty(),
        "the unfloored query hits: {unfloored}"
    );
    // The seeded memory carries no snapshot, so its hit reports an empty
    // `entities` array. The key is always present either way.
    for hit in hits(&unfloored) {
        assert!(
            hit["entities"].is_array(),
            "entities is an array on every hit: {hit}"
        );
    }
    assert!(
        unfloored.get("entity_summary").is_none(),
        "a plain recall carries no summary: {unfloored}"
    );

    let floored = recall_json(
        &db,
        &[
            "recall",
            "terse",
            "--min-final",
            UNREACHABLE_FINAL_FLOOR,
            "--output",
            "json",
        ],
    );
    assert!(
        hits(&floored).is_empty(),
        "the floor must empty the page: {floored}"
    );
    assert_eq!(
        floored["ok"],
        Value::Bool(true),
        "an empty page is a success, not a failure: {floored}"
    );
    assert!(
        floored.get("entity_summary").is_none(),
        "a floored recall carries no summary either: {floored}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Each entity control changes only the entity surface. `--no-entities` empties
/// every hit's `entities` and leaves off the derived `entity_match` reason;
/// `--entity-summary` adds the top-level roll-up object and nothing else.
///
/// `remember` stores the text as a gist and writes no snapshot, so the entity
/// names on these seeded hits are always empty. That is why the summary object
/// here is empty: it is still present, which is what the flag controls. The
/// name-ascending ordering of a non-empty summary is covered over seeded items
/// that do carry a snapshot entity, not over a `remember` fixture.
#[test]
fn recall_entity_controls_change_only_the_entity_surface() {
    let dir = seed_scenario("entities");
    let db = dir.join("clio.db");

    let baseline = recall_json(&db, &["recall", "terse", "--output", "json"]);
    let baseline_hits = hits(&baseline).to_vec();
    assert!(!baseline_hits.is_empty(), "the query hits: {baseline}");

    let suppressed = recall_json(
        &db,
        &["recall", "terse", "--no-entities", "--output", "json"],
    );
    let suppressed_hits = hits(&suppressed);
    assert_eq!(
        suppressed_hits.len(),
        baseline_hits.len(),
        "suppressing names does not change the page: {suppressed}"
    );
    for hit in suppressed_hits {
        assert_eq!(hit["entities"], serde_json::json!([]), "{hit}");
        assert!(
            hit.get("entity_match").is_none(),
            "no derived reason without names: {hit}"
        );
    }
    assert!(
        suppressed.get("entity_summary").is_none(),
        "no summary is produced without the flag: {suppressed}"
    );

    let summarized = recall_json(
        &db,
        &["recall", "terse", "--entity-summary", "--output", "json"],
    );
    let summary = summarized
        .get("entity_summary")
        .unwrap_or_else(|| panic!("the summary key is present: {summarized}"));
    assert!(
        summary.is_object(),
        "the summary is an object: {summarized}"
    );
    assert_eq!(
        summary.as_object().expect("summary object").keys().count(),
        0,
        "these seeded items carry no entity name: {summarized}"
    );
    assert_eq!(
        hits(&summarized).len(),
        baseline_hits.len(),
        "asking for a summary does not change the page: {summarized}"
    );
    for hit in hits(&summarized) {
        assert!(hit["entities"].is_array(), "per-hit names stay: {hit}");
    }

    let _ = std::fs::remove_dir_all(&dir);
}

/// The two entity controls cannot be combined: a summary is made of names, so
/// asking for one with names suppressed is refused by the tool instead of
/// silently dropping either control. The call fails closed, with no payload.
#[test]
fn recall_entity_summary_with_no_entities_is_refused() {
    let dir = seed_scenario("contradiction");
    let db = dir.join("clio.db");
    let output = run(
        &db,
        &[
            "recall",
            "terse",
            "--no-entities",
            "--entity-summary",
            "--output",
            "json",
        ],
    );
    assert!(!output.status.success(), "the call must not succeed");
    assert!(
        stdout_of(&output).is_empty(),
        "no payload: {}",
        stdout_of(&output)
    );
    let envelope = json_stderr(&output);
    assert_eq!(envelope["ok"], Value::Bool(false), "{envelope}");
    let message = envelope["message"].as_str().expect("error message");
    // The message names the two flags the caller typed, not the library's own
    // field names, which is what reaches a shell.
    assert!(message.contains("--entity-summary"), "{envelope}");
    assert!(message.contains("--no-entities"), "{envelope}");

    let _ = std::fs::remove_dir_all(&dir);
}

/// A floor value the CLI cannot parse fails with the same usage-error shape and
/// exit code as any other bad flag value, and it never reaches the tool.
#[test]
fn recall_bad_floor_value_is_a_usage_error() {
    let dir = seed_scenario("bad-value");
    let db = dir.join("clio.db");

    let bad = run(
        &db,
        &["recall", "terse", "--min-final", "high", "--output", "json"],
    );
    let bad_limit = run(
        &db,
        &["recall", "terse", "--limit", "abc", "--output", "json"],
    );
    assert_eq!(
        bad.status.code(),
        bad_limit.status.code(),
        "a bad floor value fails like any other bad flag value"
    );
    assert!(stdout_of(&bad).is_empty(), "{}", stdout_of(&bad));

    let envelope = json_stderr(&bad);
    let reference = json_stderr(&bad_limit);
    assert_eq!(envelope["ok"], reference["ok"], "{envelope}");
    assert_eq!(envelope["code"], reference["code"], "{envelope}");
    let message = envelope["message"].as_str().expect("error message");
    assert!(message.contains("--min-final"), "{envelope}");
    assert!(message.contains("high"), "{envelope}");

    // A negative floor is refused too, and named.
    let negative = run(
        &db,
        &[
            "recall",
            "terse",
            "--min-semantic",
            "-0.5",
            "--output",
            "json",
        ],
    );
    assert!(
        !negative.status.success(),
        "a negative floor must not succeed"
    );
    let negative_envelope = json_stderr(&negative);
    assert_eq!(
        negative_envelope["code"], reference["code"],
        "{negative_envelope}"
    );
    assert!(
        negative_envelope["message"]
            .as_str()
            .expect("error message")
            .contains("--min-semantic"),
        "{negative_envelope}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
