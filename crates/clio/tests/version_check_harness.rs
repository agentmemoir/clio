// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module proves the `version` verb's process contract over the real `clio`
//! binary: that the plain line is unchanged, that `--check` prints exactly one
//! JSON line over a home the test owns, and that a flag without its pair is a
//! usage refusal.
//!
//! ## Owns
//! - The identity line's bytes, as a reader sees them
//! - The one-line JSON contract at the process boundary, with the recorded
//!   skill version the marker states
//! - The unavailable answer for a home holding no marker, and its exit code
//!
//! ## Does not own
//! - The check's own logic and its request (`version_check_tests`)
//! - The binary's dispatch table (`main`)
//!
//! ## Boundary
//!
//! Tests here break when the verb's process-visible contract changes; they never
//! change it. Every child runs with `HOME` pointing at a directory this test
//! created, so no test reads or writes the caller's home.
//!
//! Keep this module focused on the version verb's process contract.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

/// A directory this test owns, removed when the guard drops.
struct Scratch {
    root: PathBuf,
}

impl Scratch {
    fn new(label: &str) -> Scratch {
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the clock is readable")
            .as_nanos();
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "clio-version-harness-{label}-{}-{nanos}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("the scratch directory can be created");
        Scratch { root }
    }

    fn home(&self) -> &Path {
        &self.root
    }

    /// Install the marker a completed installation leaves behind, recording
    /// `package_version` as the skill version.
    fn install_marker(&self, package_version: &str) {
        let package = self.root.join(".agents/skills/clio-mcp");
        std::fs::create_dir_all(&package).expect("the package directory can be created");
        let marker = format!(
            "{{\"installer\":\"clio\",\"package_version\":\"{package_version}\",\
             \"catalog_hash\":\"fnv1a64:abcd\",\"payload_digest\":\"\",\"files\":{{}}}}"
        );
        std::fs::write(package.join(".clio-skill.json"), marker).expect("the marker is written");
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Run `clio` with `args` under `home`, with no inherited environment.
fn clio(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_clio"))
        .args(args)
        .env_clear()
        .env("HOME", home)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn clio")
}

/// The stdout of `output` as text, with the child's stderr in the message when
/// it fails to be UTF-8.
fn stdout_of(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("the child writes UTF-8")
}

/// The identity line is exactly `clio <version>`, and nothing else is added.
#[test]
fn the_plain_version_line_is_unchanged() {
    let scratch = Scratch::new("plain");
    let output = clio(scratch.home(), &["version"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        stdout_of(&output),
        format!("clio {}\n", env!("CARGO_PKG_VERSION"))
    );
}

/// `version --check` over a home with a valid marker prints exactly one JSON
/// line, reporting the version the marker recorded rather than the binary's own,
/// and exits 0 or 1 according to the release answer.
#[test]
fn version_check_prints_one_json_line_over_a_valid_marker() {
    let scratch = Scratch::new("check");
    scratch.install_marker("0.0.7");
    let output = clio(scratch.home(), &["version", "--check"]);
    let stdout = stdout_of(&output);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 1, "exactly one line: {stdout:?}");
    let report: serde_json::Value = serde_json::from_str(lines[0]).expect("the line is JSON");
    let mut keys: Vec<&str> = Vec::new();
    for key in [
        "skillVersion",
        "catalogHash",
        "releaseStatus",
        "latestVersion",
    ] {
        assert!(
            report.get(key).is_some(),
            "the line carries {key}: {stdout}"
        );
        keys.push(key);
    }
    assert_eq!(
        keys,
        [
            "skillVersion",
            "catalogHash",
            "releaseStatus",
            "latestVersion"
        ]
    );
    assert_eq!(
        report["skillVersion"], "0.0.7",
        "the recorded skill version, not the binary's: {stdout}"
    );
    assert_eq!(report["catalogHash"], "fnv1a64:abcd");
    let status = report["releaseStatus"].as_str().expect("a status");
    let code = output.status.code();
    if status == "unavailable" {
        assert_eq!(report["latestVersion"], serde_json::Value::Null, "{stdout}");
        assert_eq!(code, Some(1), "unavailable exits 1: {stdout}");
    } else {
        assert!(
            status == "current" || status == "newer",
            "an established answer is current or newer: {stdout}"
        );
        assert!(report["latestVersion"].is_string(), "{stdout}");
        assert_eq!(code, Some(0), "current and newer exit 0: {stdout}");
    }
}

/// A home with no marker is unavailable, which prints the line with nulls and
/// exits 1 rather than guessing.
#[test]
fn version_check_without_a_marker_is_unavailable() {
    let scratch = Scratch::new("no-marker");
    let output = clio(scratch.home(), &["version", "--check"]);
    let stdout = stdout_of(&output);
    assert_eq!(
        stdout.trim(),
        "{\"skillVersion\":null,\"catalogHash\":null,\"releaseStatus\":\"unavailable\",\"latestVersion\":null}",
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.status.code(), Some(1));
}

/// `--harness` without `--check` is a usage refusal at the process boundary.
#[test]
fn an_unpaired_harness_flag_exits_two() {
    let scratch = Scratch::new("unpaired");
    let output = clio(scratch.home(), &["version", "--harness", "claude"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("--check"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
