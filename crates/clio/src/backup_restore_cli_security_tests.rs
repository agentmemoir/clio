// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing that a restore's report carries no
//! memory content, no subject identifier, and no key-store byte: the security
//! acceptance condition the prompt, the JSON payload, and the text view share.
//!
//! ## Owns
//! - The planted-secret restore case and the printable-run key-store check
//!
//! ## Does not own
//! - The restore sequence and its refusals (see `backup_restore_flow_tests`)
//! - The command surface (see `backup_restore_cli_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;
use crate::backup_cli_fixtures::{Scratch, key_store_path, run_json_in, snapshot};
use serde_json::json;
use std::path::{Path, PathBuf};

/// The default target store inside `scratch`.
fn store_path(scratch: &Scratch) -> PathBuf {
    scratch.join("clio.db")
}

/// Every printable run of at least `MIN_RUN` bytes in a file.
///
/// A key store's bytes are binary, but its subject ids and base64 keys are
/// printable runs inside it; searching for each run proves no stored byte
/// reached the report without this test having to parse the file.
fn printable_runs(path: &Path) -> Vec<String> {
    const MIN_RUN: usize = 16;
    let bytes = std::fs::read(path).expect("read the key store");
    let mut runs = Vec::new();
    let mut current = String::new();
    for byte in bytes {
        if byte.is_ascii_graphic() {
            current.push(byte as char);
            continue;
        }
        if current.len() >= MIN_RUN {
            runs.push(std::mem::take(&mut current));
        } else {
            current.clear();
        }
    }
    if current.len() >= MIN_RUN {
        runs.push(current);
    }
    runs
}

#[test]
fn no_planted_plaintext_or_key_byte_reaches_a_restore_report() {
    let scratch = Scratch::new("restore-secret");
    let id = snapshot(&scratch, 2);
    let key_path = key_store_path(&store_path(&scratch));
    assert!(key_path.is_file(), "the fixture wrote a key store");
    let runs = printable_runs(&key_path);
    assert!(!runs.is_empty(), "the key store has printable material");

    let (code, payload, err) = run_json_in(&scratch, &["restore", &id, "--yes"]);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert_eq!(payload["result"], json!("restored"), "{payload}");

    let report = format!("{payload} {err}");
    assert!(
        !report.contains("PLANTED-SECRET"),
        "the planted plaintext reached the report: {report}"
    );
    assert!(
        !report.contains("subject-0"),
        "a subject identifier reached the report: {report}"
    );
    for run in runs {
        assert!(
            !report.contains(&run),
            "key-store bytes reached the report: {run}"
        );
    }
}
