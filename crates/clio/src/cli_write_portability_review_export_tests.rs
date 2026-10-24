// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `compliance review-export` binding at its own layer:
//! `cli_write_portability::run_review_export` decodes the record's flags, opens
//! the store, dispatches the in-process `review_export` tool, and renders the
//! success line, and it fails closed with exit 2 on a missing flag.
//!
//! ## Owns
//! - The flag-decode and dispatch path of `run_review_export`
//! - The usage failure that names the missing `--out`
//!
//! ## Does not own
//! - Review-event semantics and destination custody (see `clio-compliance`)
//! - The whole-command wiring (see `compliance_cli_review_export_tests.rs`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::cli_output::OutputMode;
use crate::cli_read::CliIo;
use crate::cli_read_test_support::tokens;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// A unique scratch directory, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock is set")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "clio-portability-review-{label}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).expect("create scratch dir");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Run one `review-export` invocation against injected streams and no env keys.
fn run(args: &[&str]) -> (ExitCode, String, String) {
    let env = |_key: &str| -> Option<String> { None };
    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut input = std::io::Cursor::new(Vec::new());
    let code = {
        let mut io = CliIo {
            out: &mut out,
            err: &mut err,
            stdin: &mut input,
            is_tty: false,
            no_color: true,
            env: &env,
        };
        super::run_review_export(&mut io, OutputMode::Text, tokens(args))
    };
    (
        code,
        String::from_utf8(out).expect("utf8 out"),
        String::from_utf8(err).expect("utf8 err"),
    )
}

/// The command opens the store, dispatches the tool, and names the destination.
#[test]
fn review_export_opens_the_store_and_names_the_destination() {
    let scratch = Scratch::new("writes");
    let store = scratch.path().join("store.db");
    let dest = scratch.path().join("review.txt");
    let (code, out, err) = run(&[
        "--out",
        &dest.display().to_string(),
        "--actor",
        "harness",
        "--db",
        &store.display().to_string(),
        "-o",
        "json",
    ]);
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    let payload: Value = serde_json::from_str(out.trim()).expect("json payload");
    assert_eq!(payload["ok"], Value::Bool(true), "{payload}");
    assert_eq!(
        payload["destination"],
        Value::String(dest.display().to_string()),
        "{payload}"
    );
    assert!(dest.exists(), "the review file is written");
}

/// `--out` is required, and the failure is a usage failure with exit 2.
#[test]
fn review_export_without_out_is_a_usage_failure() {
    let (code, out, err) = run(&[]);
    assert_eq!(code, ExitCode::from(2), "{out}{err}");
    assert!(err.contains("--out"), "{err}");
    assert!(
        out.is_empty(),
        "a usage failure writes nothing to stdout: {out}"
    );
}
