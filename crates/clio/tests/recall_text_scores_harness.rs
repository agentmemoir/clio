// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the recall text view's per-stage score breakdown against
//! the real `clio` binary: it spawns `remember` to seed a database, then spawns
//! `recall` in text and JSON mode and checks the rendered line, the retained
//! relative-score disclaimer, the absent-arm placeholder, the unchanged empty
//! output, and the fact that JSON never carries a text-view line.
//!
//! ## Owns
//! - End-to-end recall-text breakdown assertions over spawned `clio` processes
//!
//! ## Does not own
//! - The breakdown's formatting over synthetic payloads
//!   (`cli_read_render_scores_tests`) and over the in-process CLI path
//!   (`cli_read_output_tests`)
//! - The `-o`/`--output` alias contract (`output_alias_harness`)
//! - The CLI implementation under test
//!
//! ## Boundary
//!
//! Tests here break when the real binary's recall text stops carrying the
//! breakdown; they never change it.
//!
//! Keep this module focused on the spawned-binary recall text contract.

mod common;

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

/// Calls served so far in this test binary, so `temp_dir` never returns a
/// directory another scenario already created.
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// The note the recall text view must keep above the hits.
const RELATIVE_SCORE_NOTE: &str = "score is relative (rank fusion), not calibrated relevance";

/// The text a hit with only a lexical arm shows: the dense and rerank arms never
/// scored it, so both show the placeholder.
const EXPECTED_ABSENT_ARMS: &str = "semantic -  keyword ";

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
        "clio-recall-scores-{tag}-{}-{nanos}-{sequence}",
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

/// The numeric value a breakdown line shows for one arm, read back from the text.
fn arm_value<'a>(line: &'a str, arm: &str) -> Option<&'a str> {
    let mut tokens = line.split_whitespace().skip_while(|token| *token != arm);
    tokens.next()?;
    tokens.next()
}

/// The per-stage breakdown line for one hit, taken from a rendered recall text.
fn breakdown_line(text: &str) -> &str {
    text.lines()
        .find(|line| line.trim_start().starts_with("scores: final"))
        .unwrap_or_else(|| panic!("no breakdown line in the recall text: {text}"))
}

#[test]
fn recall_text_shows_the_stage_breakdown_end_to_end() {
    let dir = temp_dir("breakdown");
    let db = dir.join("clio.db");

    // Seed through the real binary; a CLI write drains the index, so the next
    // process can recall it.
    let seed = run(
        &db,
        &[
            "remember",
            "terse answers beat verbose replies",
            "--category",
            "tool_config",
        ],
    );
    assert!(seed.status.success(), "seed failed: {}", stderr_of(&seed));

    // Text mode: the disclaimer, the provenance reason, and the breakdown line.
    let output = run(&db, &["recall", "terse", "--output", "text"]);
    assert!(output.status.success(), "{}", stderr_of(&output));
    let text = stdout_of(&output);
    assert!(text.contains("Results for: terse"), "{text}");
    assert!(text.contains(RELATIVE_SCORE_NOTE), "{text}");
    assert!(text.contains("[lexical match]"), "{text}");

    let line = breakdown_line(&text);
    assert!(line.contains(EXPECTED_ABSENT_ARMS), "{text}");
    assert!(line.contains("reranker -"), "{text}");
    // An absent arm is never shown as a number, not even a zero.
    for absent in ["semantic", "reranker"] {
        assert!(
            !line.contains(&format!("{absent} 0")),
            "{absent} shown as a number: {text}"
        );
    }

    // The lexical arm is present, positive, and printed at full precision: a
    // four-decimal format would render it as `0.0000`, which is the failure the
    // placeholder rule exists to prevent.
    let keyword = arm_value(line, "keyword")
        .unwrap_or_else(|| panic!("the keyword arm must be shown: {text}"));
    let keyword_value: f64 = keyword
        .parse()
        .unwrap_or_else(|e| panic!("the keyword arm must be numeric ({keyword}): {e}"));
    assert!(keyword_value > 0.0, "keyword must not be zero: {text}");
    assert_ne!(
        format!("{keyword_value:.4}"),
        keyword,
        "the arm must print the exact value, not a 4-decimal rounding: {text}"
    );

    // The fused value appears on the headline too, and the two agree.
    let headline = text
        .lines()
        .find(|line| line.trim_start().starts_with("1."))
        .unwrap_or_else(|| panic!("first hit: {text}"));
    let final_value =
        arm_value(line, "final").unwrap_or_else(|| panic!("the final arm must be shown: {text}"));
    assert!(
        headline.ends_with(&format!("score {final_value}")),
        "headline and breakdown agree: {headline} vs {line}"
    );

    // JSON mode: the payload keeps its shape, keeps the score object, and never
    // carries a text-view line.
    let output = run(&db, &["recall", "terse", "--output", "json"]);
    assert!(output.status.success(), "{}", stderr_of(&output));
    let stdout = stdout_of(&output);
    assert!(!stdout.contains("scores: final"), "{stdout}");
    assert!(!stdout.contains(RELATIVE_SCORE_NOTE), "{stdout}");
    let payload = json_stdout(&output);
    let hits = payload["hits"].as_array().expect("hits");
    assert!(!hits.is_empty(), "{payload}");
    let scores = hits[0]["scores"].as_object().expect("scores object");
    let mut keys: Vec<&str> = scores.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "age_factor",
            "final",
            "keyword",
            "reranker",
            "semantic",
            "trust_factor"
        ],
        "{payload}"
    );
    assert!(scores["final"].is_number(), "{payload}");
    assert!(scores["keyword"].is_number(), "{payload}");
    // No embedder and no reranker in this deployment, so both arms stay null and
    // the text view shows the placeholder rather than a zero.
    assert!(scores["semantic"].is_null(), "{payload}");
    assert!(scores["reranker"].is_null(), "{payload}");

    // Empty recall output is unchanged.
    let output = run(&db, &["recall", "zzz-nothing", "--output", "text"]);
    assert!(output.status.success(), "{}", stderr_of(&output));
    assert_eq!(stdout_of(&output).trim(), "no results");

    let _ = std::fs::remove_dir_all(&dir);
}
