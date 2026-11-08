// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module proves the `version` verb's process contract over the real `clio`
//! binary: that the identity line is unchanged for all three spellings, that a
//! check over a home holding no marker answers unavailable, and that a flag
//! without its pair is a usage refusal.
//!
//! ## Owns
//! - The identity line's bytes, as a reader sees them, for `version`,
//!   `--version` and `-V`
//! - The one-line JSON contract at the process boundary when no installation
//!   evidence exists, and the nulls it carries
//! - The unavailable answer's exit code, and that it writes nothing to stderr
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
//! No test here owns an endpoint, and none may open a socket: the production
//! endpoint is hard-wired inside the binary, so a home with no marker is the only
//! evidence state a process test can drive without a request. The skipped request
//! is proven where the endpoint is an argument instead: `version_check_tests`
//! counts what its own loopback listener receives.
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

/// The identity line is exactly `clio <version>`, and nothing else is added, for
/// the verb and for both of its flags.
#[test]
fn the_plain_version_line_is_unchanged() {
    let scratch = Scratch::new("plain");
    for args in [&["version"][..], &["--version"][..], &["-V"][..]] {
        let output = clio(scratch.home(), args);
        assert_eq!(
            output.status.code(),
            Some(0),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            stdout_of(&output),
            format!("clio {}\n", env!("CARGO_PKG_VERSION")),
            "{args:?}"
        );
    }
}

/// A home with no marker is unavailable, which prints one line of four keys with
/// nulls, exits 1, and leaves stderr empty. No request is made, because there is
/// no installed version to compare a release against.
#[test]
fn version_check_without_a_marker_is_unavailable() {
    let scratch = Scratch::new("no-marker");
    let output = clio(scratch.home(), &["version", "--check"]);
    let stdout = stdout_of(&output);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 1, "exactly one line: {stdout:?}");
    assert_eq!(
        lines[0],
        "{\"skillVersion\":null,\"catalogHash\":null,\
         \"releaseStatus\":\"unavailable\",\"latestVersion\":null}",
        "the four keys, in order, with nulls"
    );
    let report: serde_json::Value = serde_json::from_str(lines[0]).expect("the line is JSON");
    for key in [
        "skillVersion",
        "catalogHash",
        "releaseStatus",
        "latestVersion",
    ] {
        assert!(report.get(key).is_some(), "the line carries {key}");
    }
    assert_eq!(report["releaseStatus"], "unavailable");
    for key in ["skillVersion", "catalogHash", "latestVersion"] {
        assert_eq!(report[key], serde_json::Value::Null, "{key} is null");
    }
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "",
        "the line goes to stdout only"
    );
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
