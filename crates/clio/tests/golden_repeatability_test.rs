// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module proves the golden fixture's content is repeatable and free of
//! identity metadata: child-only COLUMNS agreement, byte-equal repeated
//! captures of one build, the fixture's exact field set, the frozen fixture
//! against the live binary, and the env-gated capture mode that freezes it.
//!
//! ## Owns
//! - Every repeatability and identity assertion over the golden fixture
//! - The capture-mode entry point that freezes the fixture on request
//!
//! ## Does not own
//! - The capture tooling itself (`golden_capture_support`)
//! - The guard-verdict and comparator unit tests (`golden_capture_test`)
//!
//! ## Boundary
//!
//! Changes to what makes a capture repeatable should normally require changes
//! here. Changes to how captures are bounded or guarded should not.
//!
//! Keep this module focused on golden-capture repeatability.

use serde_json::Value;
use std::path::Path;

#[path = "golden_capture_support/mod.rs"]
mod golden_capture_support;

use golden_capture_support::{
    CAPTURE_ENV, CaptureEntry, ContentDiff, FIXTURE_VERSION, GoldenFixture, capture, capture_plan,
    compare, fixture_path, guarded_build, load_fixture, source_identity, source_mutated,
    write_fixture,
};

/// Capture every command in `plan` from `binary`, with `columns` applied to
/// each child only. Every child is bounded and reaped by the support runner.
fn captured_plan(binary: &Path, plan: &[Vec<String>], columns: Option<&str>) -> Vec<CaptureEntry> {
    plan.iter()
        .map(|argv| capture(binary, argv, columns).expect("bounded capture"))
        .collect()
}

/// The sorted key set of a serialized JSON object.
fn object_keys(value: &Value) -> Vec<String> {
    let mut keys: Vec<String> = value
        .as_object()
        .expect("serialized record must be a JSON object")
        .keys()
        .cloned()
        .collect();
    keys.sort();
    keys
}

/// The help renderer ignores COLUMNS: with the variable unset on the child
/// and with widths 100 and 200 set on the child only, three captures of the
/// full plan agree entry for entry, content bytes included.
#[test]
fn columns_unset_100_and_200_produce_identical_captures() {
    let binary = Path::new(env!("CARGO_BIN_EXE_clio"));
    let plan = capture_plan(binary);
    let unset = captured_plan(binary, &plan, None);
    let narrow = captured_plan(binary, &plan, Some("100"));
    let wide = captured_plan(binary, &plan, Some("200"));
    assert_eq!(
        unset, narrow,
        "COLUMNS=100 on the child must not change the capture"
    );
    assert_eq!(
        unset, wide,
        "COLUMNS=200 on the child must not change the capture"
    );
}

/// Two captures of the same build with identical arguments are byte-identical,
/// entry for entry.
#[test]
fn two_captures_of_the_same_build_are_byte_identical() {
    let binary = Path::new(env!("CARGO_BIN_EXE_clio"));
    let plan = capture_plan(binary);
    let first = captured_plan(binary, &plan, None);
    let second = captured_plan(binary, &plan, None);
    assert_eq!(first, second, "repeated captures must agree byte for byte");
}

/// The fixture serializes exactly `version` and `entries`, an entry exactly
/// argv/exit/stdout/stderr, and the source identity never appears inside any
/// captured output: identity is metadata, never command output.
#[test]
fn identity_metadata_stays_out_of_the_fixture_content() {
    let entry = CaptureEntry {
        argv: vec!["help".to_owned()],
        exit: Some(0),
        stdout: "usage text".to_owned(),
        stderr: String::new(),
    };
    let fixture = GoldenFixture {
        version: FIXTURE_VERSION,
        entries: vec![entry.clone()],
    };
    assert_eq!(
        object_keys(&serde_json::to_value(&fixture).expect("fixture serializes")),
        vec!["entries", "version"],
        "the fixture must hold exactly version and entries"
    );
    assert_eq!(
        object_keys(&serde_json::to_value(&entry).expect("entry serializes")),
        vec!["argv", "exit", "stderr", "stdout"],
        "an entry must carry argv, exit, stdout, and stderr only"
    );

    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let identity = source_identity(&root).expect("the workspace is a git checkout");
    let binary = Path::new(env!("CARGO_BIN_EXE_clio"));
    for captured in captured_plan(binary, &capture_plan(binary), None) {
        assert!(
            !captured.stdout.contains(&identity) && !captured.stderr.contains(&identity),
            "command {:?} must not print the source identity",
            captured.argv
        );
    }
}

/// The frozen fixture matches the live binary command for command; a failure
/// prints one report line per difference, naming the exact command and the
/// changed output.
#[test]
fn fixture_matches_the_live_binary_command_for_command() {
    let path = fixture_path();
    assert!(
        path.is_file(),
        "the golden fixture is not frozen yet: run the capture mode to write {}",
        path.display()
    );
    let fixture = load_fixture(&path).expect("the frozen fixture parses");
    let binary = Path::new(env!("CARGO_BIN_EXE_clio"));
    let captured = captured_plan(binary, &capture_plan(binary), None);
    let diffs = compare(&fixture, &captured);
    assert!(
        diffs.is_empty(),
        "the live binary drifted from the fixture:\n{}",
        diffs
            .iter()
            .map(ContentDiff::report)
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// Capture mode freezes the fixture only when `CAPTURE_ENV` is exactly "1";
/// the variable is read here and never written by tests. CI never sets it, so
/// the unset branch is what every normal run exercises. The capture branch is
/// a developer action run by hand.
#[test]
fn capture_mode_writes_the_fixture_only_when_asked() {
    let path = fixture_path();
    let existed_before = path.exists();
    if std::env::var(CAPTURE_ENV).as_deref() != Ok("1") {
        assert_eq!(
            path.exists(),
            existed_before,
            "the unset branch must not touch the fixture file"
        );
        return;
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let identity_before = source_identity(&root).expect("the workspace is a git checkout");
    let executable = guarded_build(&root)
        .unwrap_or_else(|error| panic!("the guarded release build refused: {error}"));
    let binary = Path::new(&executable);
    let captured = captured_plan(binary, &capture_plan(binary), None);
    let identity_after = source_identity(&root).expect("the workspace is a git checkout");
    assert!(
        !source_mutated(&identity_before, &identity_after),
        "the source changed during capture: {identity_before} -> {identity_after}"
    );
    let fixture = GoldenFixture {
        version: FIXTURE_VERSION,
        entries: captured,
    };
    write_fixture(&fixture, &path).expect("write the frozen fixture");
    println!(
        "froze {} commands from {} into {}",
        fixture.entries.len(),
        executable,
        path.display()
    );
}
