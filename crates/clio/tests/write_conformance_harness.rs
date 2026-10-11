// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module proves the write heads kept their original surfaces after the
//! registry migration: one catalog record per write verb with byte-identical
//! help, the text/file and file/stdin alternate forms, the parser's refusal
//! texts, and the confirmation gate over a disposable store.
//!
//! ## Owns
//! - The single-record and original-bytes assertions over the write heads
//! - The alternate-form assertions for `admit --file` and `batch --file -`
//! - The parser-refusal text pins for unknown, missing, and repeated flags
//! - The confirmation-gate assertions over a disposable store
//!
//! ## Does not own
//! - The frozen fixture and its comparator (`golden_capture_support`)
//! - The whole-surface and repeatability golden tests (`golden_repeatability_test`)
//! - The read-side `-o` alias contract (`output_alias_harness`)
//!
//! ## Boundary
//!
//! Tests here break when a write head's catalog record, help bytes, parser
//! refusals, or confirmation gate change; they never change it.
//!
//! Keep this module focused on write-head conformance against the frozen
//! golden capture.

#[path = "golden_capture_support/mod.rs"]
mod golden_capture_support;

mod common;

use golden_capture_support::{CaptureEntry, capture, fixture_path, load_fixture};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

/// The thirteen migrated write-head verbs: the eight core writes and the five
/// write operations, in registry tranche order.
const WRITE_HEADS: &[&str] = &[
    "remember",
    "admit",
    "summarize",
    "consolidate",
    "update",
    "invalidate",
    "discard",
    "correct",
    "validate",
    "batch",
    "export",
    "import",
    "erase",
];

/// The write verbs whose records live in the read tranches: one record is
/// shared by the group's help and its parser, so the catalog must list each
/// exactly once.
const GROUP_WRITES: &[&str] = &[
    "task upsert",
    "persona stable",
    "persona observe",
    "shared store",
    "shared discard",
    "triple add",
    "triple end",
    "belief observe",
    "graph link",
    "scratchpad write",
    "scratchpad clear",
    "canonical put",
    "hygiene clean",
];

/// Calls served so far in this test binary, so `temp_dir` never returns a
/// directory another scenario already created. Same pattern as
/// `exact_duplicate_harness`.
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Unique temp directory for one scenario, cleaned on drop of the returned
/// guard's lifetime by the caller's `remove_dir_all`.
fn temp_dir(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "clio-write-conformance-{tag}-{}-{nanos}-{sequence}",
        std::process::id()
    ));
    fs::create_dir_all(&dir).expect("temp dir");
    dir
}

/// The `clio` binary with a cleared environment plus the coverage pattern
/// (`exact_duplicate_harness::clio`).
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

/// Parse one command's stdout as JSON, with the raw streams on failure.
fn json_stdout(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "stdout is not JSON: {error}\nstdout: {}\nstderr: {}",
            stdout_of(output),
            stderr_of(output)
        )
    })
}

/// The `{"code","message","hint"}` refusal object on stderr.
fn refusal_of(output: &Output) -> Value {
    serde_json::from_str(stderr_of(output).trim())
        .unwrap_or_else(|error| panic!("stderr is not a refusal: {error}\n{}", stderr_of(output)))
}

/// The frozen fixture entry whose argv equals `argv`, with its index.
fn fixture_entry(fixture: &golden_capture_support::GoldenFixture, argv: &[&str]) -> CaptureEntry {
    let wanted: Vec<String> = argv.iter().map(|token| (*token).to_owned()).collect();
    let matches: Vec<&CaptureEntry> = fixture
        .entries
        .iter()
        .filter(|entry| entry.argv == wanted)
        .collect();
    assert_eq!(
        matches.len(),
        1,
        "the fixture must hold exactly one entry for {argv:?}"
    );
    matches[0].clone()
}

/// The live binary's capture of one command, byte-compared to its fixture
/// entry: exit, stdout, and stderr must all match.
fn live_entry_matches_fixture(argv: &[&str]) {
    let fixture = load_fixture(&fixture_path()).expect("the frozen fixture parses");
    let expected = fixture_entry(&fixture, argv);
    let actual = capture(Path::new(env!("CARGO_BIN_EXE_clio")), &expected.argv, None)
        .expect("bounded capture");
    assert_eq!(
        (&actual.exit, &actual.stdout, &actual.stderr),
        (&expected.exit, &expected.stdout, &expected.stderr),
        "command {argv:?} drifted from the frozen capture"
    );
}

/// How many times the live catalog lists `name`: once in `commands`, once in
/// `commands_without_tools`, or never. Exactly one is the single-record claim.
fn catalog_lists(catalog: &Value, name: &str) -> usize {
    let bound = catalog["commands"]
        .as_array()
        .expect("live catalog commands array")
        .iter()
        .filter(|entry| entry["command"].as_str() == Some(name))
        .count();
    let unbound = catalog["commands_without_tools"]
        .as_array()
        .expect("live catalog commands_without_tools array")
        .iter()
        .filter(|entry| entry.as_str() == Some(name))
        .count();
    bound + unbound
}

/// The live `help --json` catalog, captured from the real binary.
fn live_catalog() -> Value {
    let binary = Path::new(env!("CARGO_BIN_EXE_clio"));
    let entry = capture(binary, &["help".to_owned(), "--json".to_owned()], None)
        .expect("bounded catalog capture");
    serde_json::from_str(&entry.stdout).expect("live catalog stdout is JSON")
}

/// Every verb in `verbs` holds exactly one live catalog record and its help
/// bytes match the frozen fixture, so no record is duplicated across tranches
/// and no rendered surface drifted.
fn assert_single_record_and_original_bytes(verbs: &[&str]) {
    let catalog = live_catalog();
    for name in verbs {
        assert_eq!(
            catalog_lists(&catalog, name),
            1,
            "{name} must be listed exactly once in the live catalog"
        );
        let mut argv: Vec<&str> = name.split(' ').collect();
        argv.push("--help");
        live_entry_matches_fixture(&argv);
    }
}

/// Every migrated write head holds exactly one catalog record whose captured
/// help bytes still match the frozen fixture, and the original catalog output
/// is what the live binary prints.
#[test]
fn every_write_head_holds_one_record_with_original_bytes() {
    assert_single_record_and_original_bytes(WRITE_HEADS);
}

/// The `admit` text/file and `batch` file/stdin alternates: their usage lines
/// are in the live help bytes, and both alternates run isolated on a
/// disposable store where `admit --file` is a read-only preview.
#[test]
fn admit_and_batch_alternates_match_the_captures() {
    let binary = Path::new(env!("CARGO_BIN_EXE_clio"));
    let alternates: [([&str; 2], &str); 2] = [
        (
            ["admit", "--help"],
            "       or: clio admit --file ops.json [--bank BANK] [--actor ACTOR]",
        ),
        (
            ["batch", "--help"],
            "       or: clio batch --file - (reads operations JSON from stdin)",
        ),
    ];
    for (argv, alternate) in alternates {
        live_entry_matches_fixture(&argv);
        let full: Vec<String> = argv.iter().map(|token| (*token).to_owned()).collect();
        let live = capture(binary, &full, None).expect("bounded help capture");
        assert!(
            live.stdout.contains(alternate),
            "command {argv:?} must show its alternate form: {}",
            live.stdout
        );
    }

    let dir = temp_dir("alternates");
    let db = dir.join("clio.db");
    let ops = dir.join("ops.json");
    fs::write(
        &ops,
        r#"{"items":[{"item":{"gist":"file admitted fact"},"category":"persona","epistemic_kind":"fact"}]}"#,
    )
    .expect("write the admission file");
    let output = run(&db, &["admit", "--file", &ops.to_string_lossy()]);
    assert!(output.status.success(), "{}", stderr_of(&output));
    let payload = json_stdout(&output);
    assert_eq!(payload["read_only"], Value::Bool(true), "{payload}");
    assert_eq!(payload["writes"], Value::Number(0.into()), "{payload}");

    let batch = dir.join("batch.json");
    fs::write(&batch, r#"{"operations":[]}"#).expect("write the batch file");
    let stdin = fs::File::open(&batch).expect("open the batch file as stdin");
    let output = clio()
        .args(["batch", "--file", "-"])
        .arg("--db")
        .arg(&db)
        .stdin(Stdio::from(stdin))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn clio");
    assert!(output.status.success(), "{}", stderr_of(&output));
    assert_eq!(json_stdout(&output)["ok"], Value::Bool(true));

    assert!(
        fs::remove_dir_all(&dir).is_ok(),
        "the scenario directory is disposable"
    );
}

/// The write parser's refusals keep their original text and exit code: an
/// unknown flag, a missing required flag, a repeated non-repeatable flag, and
/// the global short output form, plus its accepted spelling.
#[test]
fn write_parser_refusals_keep_the_original_text() {
    let dir = temp_dir("parser");
    let db = dir.join("clio.db");

    let refused = |args: &[&str], message: &str, hint: Option<&str>| {
        let output = run(&db, args);
        assert_eq!(output.status.code(), Some(2), "{}", stderr_of(&output));
        let refusal = refusal_of(&output);
        assert_eq!(refusal["code"], "usage", "{refusal}");
        assert_eq!(refusal["message"], message, "{refusal}");
        match hint {
            Some(hint) => assert_eq!(refusal["hint"], hint, "{refusal}"),
            None => assert_eq!(
                refusal["help"]
                    .as_str()
                    .map(|text| text.starts_with("Usage: clio admit")),
                Some(true),
                "{refusal}"
            ),
        }
    };

    refused(
        &["admit", "hello", "--nope", "--category", "x"],
        "unknown flag `--nope`",
        Some("did you mean `--no-input`?"),
    );
    refused(
        &["admit", "hello"],
        "missing required option '--category <value>'",
        None,
    );
    refused(
        &["admit", "hello", "--category", "a", "--category", "b"],
        "--category was given more than once",
        Some("pass --category once"),
    );
    refused(
        &["remember", "hello", "--category", "x", "-o"],
        "flag `-o` requires a value",
        Some("try `-o json` or `-o text`"),
    );

    // The accepted spelling of the global short output form on a write head.
    let output = run(
        &db,
        &[
            "remember",
            "short form fact",
            "--category",
            "persona",
            "-o",
            "json",
        ],
    );
    assert!(output.status.success(), "{}", stderr_of(&output));
    assert_eq!(json_stdout(&output)["ok"], Value::Bool(true));

    fs::remove_dir_all(&dir).expect("remove the scenario directory");
}

/// The confirmation gate still guards the destructive write heads: without
/// `--confirm` both `discard` and `erase` refuse with exit 2 and the
/// disposable store keeps every item; with `--confirm` the discard runs.
#[test]
fn confirm_gated_writes_stay_guarded_on_a_disposable_store() {
    let dir = temp_dir("confirm");
    let db = dir.join("clio.db");

    let output = run(
        &db,
        &[
            "remember",
            "guarded fact",
            "--category",
            "persona",
            "--output",
            "json",
        ],
    );
    assert!(output.status.success(), "{}", stderr_of(&output));
    let stored = json_stdout(&output)["id"]
        .as_str()
        .expect("the stored item's id")
        .to_owned();

    let stats = |label: &str| {
        let output = run(&db, &["stats", "--output", "json"]);
        assert!(output.status.success(), "{label}: {}", stderr_of(&output));
        json_stdout(&output)
    };

    let output = run(&db, &["discard", &stored, "--reason", "probe"]);
    assert_eq!(output.status.code(), Some(2), "{}", stderr_of(&output));
    let refusal = refusal_of(&output);
    assert_eq!(
        refusal["message"], "`discard` is destructive and was refused without confirmation",
        "{refusal}"
    );
    let before = stats("after the guarded discard");
    assert_eq!(before["items_total"], Value::Number(1.into()), "{before}");
    assert_eq!(
        before["items_discarded"],
        Value::Number(0.into()),
        "{before}"
    );

    let output = run(
        &db,
        &[
            "erase",
            "--subject",
            "s1",
            "--basis",
            "consent-withdrawal",
            "--request",
            "r1",
        ],
    );
    assert_eq!(output.status.code(), Some(2), "{}", stderr_of(&output));
    assert_eq!(
        refusal_of(&output)["message"],
        "`erase` is destructive and was refused without confirmation",
    );

    let output = run(&db, &["discard", &stored, "--reason", "probe", "--confirm"]);
    assert!(output.status.success(), "{}", stderr_of(&output));
    let after = stats("after the confirmed discard");
    assert_eq!(after["items_discarded"], Value::Number(1.into()), "{after}");

    fs::remove_dir_all(&dir).expect("remove the scenario directory");
}

/// The write groups whose records live in the read tranches each hold exactly
/// one catalog record and their help bytes match the frozen fixture, so the
/// shared record is looked up from one source with no duplicate across
/// tranches.
#[test]
fn write_group_verbs_share_single_records_across_tranches() {
    assert_single_record_and_original_bytes(GROUP_WRITES);
}
