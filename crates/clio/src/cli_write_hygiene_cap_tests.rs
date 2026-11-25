// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the input caps `hygiene clean` puts on its candidates
//! document: the entry-count refusal before the document is copied, the capped
//! read of a named path as well as of the injected stream, and the hint the
//! byte refusal carries.
//!
//! ## Owns
//! - Cap and path-arm tests for the `hygiene clean` build path
//!
//! ## Does not own
//! - The command under test (`cli_write_hygiene`)
//! - The capped read itself (see `cli_read_cap`)
//! - End-to-end clean behavior (`cli_write_hygiene_tests.rs`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::{HygieneWriteGroup, parse_candidates};
use crate::cli_read_cap::{MAX_INPUT_BYTES, MAX_INPUT_ENTRIES};
use crate::cli_write_group::WriteGroup;
use crate::cli_write_test_support::run_write_state_stdin;
use serde_json::json;
use std::io::Cursor;
use std::process::ExitCode;

/// A disposable candidates file holding `body`.
fn candidates_file(name: &str, body: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "clio-hygiene-cap-{name}-{}.json",
        std::process::id()
    ));
    std::fs::write(&path, body).expect("write candidates file");
    path
}

/// `n` candidates with the smallest legal shape.
fn candidates(n: usize) -> String {
    let entries: Vec<_> = (0..n)
        .map(|i| json!({"item_id": format!("itm-{i}")}))
        .collect();
    json!({"candidates": entries}).to_string()
}

#[test]
fn a_document_over_the_entry_cap_is_refused_naming_the_count_and_the_cap() {
    let s = crate::cli_read_test_support::state();
    let document = candidates(MAX_INPUT_ENTRIES + 1);
    let mut reader = Cursor::new(document.into_bytes());
    let args = ["--file", "-", "--action", "flag"];
    let (code, out, err) = run_write_state_stdin("hygiene clean", &args, &mut reader, &s, false);
    assert_eq!(code, ExitCode::from(2), "{out}{err}");
    assert!(
        err.contains(&format!("carries {} candidates", MAX_INPUT_ENTRIES + 1)),
        "{err}"
    );
    assert!(
        err.contains(&format!("{MAX_INPUT_ENTRIES}-entry input cap")),
        "{err}"
    );
    assert!(err.contains("independent target"), "{err}");
}

#[test]
fn a_document_at_the_entry_cap_passes_the_count_gate() {
    let s = crate::cli_read_test_support::state();
    let document = candidates(MAX_INPUT_ENTRIES);
    let mut reader = Cursor::new(document.into_bytes());
    let args = ["--file", "-", "--action", "flag"];
    let (code, out, err) = run_write_state_stdin("hygiene clean", &args, &mut reader, &s, false);
    // The ids name no stored item, so the tool skips them; the count gate let
    // the document through, which is what this case is about.
    assert_ne!(
        code,
        ExitCode::from(2),
        "the cap refused {MAX_INPUT_ENTRIES}: {out}{err}"
    );
    assert!(!err.contains("input cap"), "{err}");
}

/// The active item count for `demo`, used to prove a refused run wrote nothing.
fn active_items(s: &clio_mcp::runtime::McpState) -> u64 {
    let report = crate::cli_read_test_support::call(s, "stats", json!({"bank": "demo"}));
    assert_eq!(report["ok"], json!(true), "{report}");
    report["items_active"].as_u64().unwrap_or(u64::MAX)
}

#[test]
fn a_candidates_file_past_the_byte_cap_is_refused_by_name() {
    let s = crate::cli_read_test_support::state();
    crate::cli_read_test_support::seed_item(&s, "demo", "itm-cap", "stale duplicated note");
    // The document names the seeded item, so a cap that failed to fire would
    // discard it and the count below would show it.
    let mut body = json!({"candidates": [{"item_id": "itm-cap"}]})
        .to_string()
        .into_bytes();
    body.resize(MAX_INPUT_BYTES + 1, b' ');
    let path = candidates_file("oversized", &String::from_utf8(body).expect("ascii"));
    let arg = format!("--file={}", path.display());
    // The flags that commit, so a cap that failed to fire before the write would
    // be caught by the discarded item rather than by a preview that writes
    // nothing however the command behaves.
    let (code, out, err) = run_write_state_stdin(
        "hygiene clean",
        &[
            &arg,
            "--action",
            "discard",
            "--confirm",
            "--reason",
            "noise",
            "--bank",
            "demo",
        ],
        &mut Cursor::new(vec![]),
        &s,
        false,
    );
    assert_eq!(code, ExitCode::from(2), "{out}{err}");
    assert!(
        err.contains(&format!("--file `{}` is larger than the", path.display())),
        "{err}"
    );
    assert_eq!(
        active_items(&s),
        1,
        "refused before any write: the seeded candidate is still active"
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_missing_candidates_file_keeps_the_not_readable_wording() {
    let s = crate::cli_read_test_support::state();
    let (code, out, err) = run_write_state_stdin(
        "hygiene clean",
        &["--file=/nonexistent/candidates.json", "--action", "flag"],
        &mut Cursor::new(vec![]),
        &s,
        false,
    );
    assert_eq!(code, ExitCode::from(2), "{out}{err}");
    assert!(
        err.contains("--file `/nonexistent/candidates.json` is not readable: "),
        "{err}"
    );
}

#[test]
fn build_without_streams_still_needs_stdin_for_the_dash_path() {
    let parsed = crate::cli_args::parse(
        &HygieneWriteGroup.spec("hygiene clean"),
        ["--file".to_owned(), "-".to_owned()],
    )
    .expect("parse");
    let Err(error) = HygieneWriteGroup.build("hygiene clean", &parsed) else {
        panic!("no stdin on this path")
    };
    assert_eq!(error.exit_code(), ExitCode::from(2));
}

#[test]
fn parse_candidates_still_rejects_an_entries_document_that_is_not_an_array() {
    let error = parse_candidates(r#"{"candidates": {}}"#).expect_err("not an array");
    assert_eq!(error.exit_code(), ExitCode::from(2));
    assert!(
        error.message().contains("must be a JSON array"),
        "{error:?}"
    );
}
