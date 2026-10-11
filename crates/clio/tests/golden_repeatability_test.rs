// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module proves the golden fixture's content is repeatable and free of
//! identity metadata: child-only COLUMNS agreement, byte-equal repeated
//! captures of one build, the fixture's exact field set, the frozen fixture
//! against the live binary both command for command and surface by surface,
//! and the env-gated capture mode that freezes it.
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

/// The frozen fixture's `ranking_env_set` description text and top-level
/// `catalog_hash` are stale by a pre-existing known drift. Pin the pardon to
/// exactly those two fields: both catalogs must still carry the description,
/// the live text replaces the frozen copy, the hash is dropped before the
/// caller compares, and every other byte of the catalog still compares.
fn pardon_known_drift(frozen: &mut Value, live: &Value) {
    let description = live["tools"]
        .as_array()
        .expect("live catalog tools array")
        .iter()
        .find(|tool| tool["name"] == "ranking_env_set")
        .expect("live catalog lists ranking_env_set")["description"]
        .as_str()
        .expect("the live ranking_env_set description is a string")
        .to_owned();
    let tool = frozen["tools"]
        .as_array_mut()
        .expect("fixture catalog tools array")
        .iter_mut()
        .find(|tool| tool["name"] == "ranking_env_set")
        .expect("fixture catalog lists ranking_env_set");
    tool["description"] = Value::String(description);
    frozen
        .as_object_mut()
        .expect("fixture catalog JSON object")
        .remove("catalog_hash");
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

/// The frozen capture's whole surface matches the live binary, surface by
/// surface and in capture order: the `help` catalog, every captured command's
/// `--help`, and the whole `help --json` catalog.
///
/// The byte-for-byte whole-capture comparison lives beside this test. This one
/// names each surface, so a failure says which one moved, and it compares the
/// `help --json` entry's stdout key by key: every top-level field, the whole
/// `tools` array included, except the two fields of a pre-existing known
/// drift (`pardon_known_drift`), and it compares that entry's exit and stderr
/// like any other entry's.
///
/// The order comes from the live catalog, not the fixture: the captured argv
/// sequence must equal the fixture's entry for entry, and the arrays' own
/// sequence invariants are asserted without consulting the fixture, so a
/// refresh cannot make a reordered or duplicated catalog compare equal.
#[test]
fn every_captured_surface_matches_the_fixture_in_capture_order() {
    let fixture = load_fixture(&fixture_path()).expect("the frozen fixture parses");
    let binary = Path::new(env!("CARGO_BIN_EXE_clio"));
    let captured = captured_plan(binary, &capture_plan(binary), None);

    let expected: Vec<&Vec<String>> = fixture.entries.iter().map(|entry| &entry.argv).collect();
    let actual: Vec<&Vec<String>> = captured.iter().map(|entry| &entry.argv).collect();
    assert_eq!(
        actual, expected,
        "the captured surface must be the fixture's surface, in the fixture's order"
    );

    for (expected, actual) in fixture.entries.iter().zip(&captured) {
        if expected.argv == ["help".to_owned(), "--json".to_owned()] {
            let mut want: Value =
                serde_json::from_str(&expected.stdout).expect("fixture catalog JSON");
            let got: Value = serde_json::from_str(&actual.stdout).expect("live catalog JSON");
            pardon_known_drift(&mut want, &got);
            let mut keys: Vec<String> = want
                .as_object()
                .expect("fixture catalog JSON object")
                .keys()
                .chain(got.as_object().expect("live catalog JSON object").keys())
                .cloned()
                .collect();
            keys.sort();
            keys.dedup();
            for key in keys {
                if key == "catalog_hash" {
                    continue;
                }
                assert_eq!(
                    got.get(key.as_str()),
                    want.get(key.as_str()),
                    "command {:?}: the live catalog's `{key}` drifted from the frozen capture",
                    expected.argv
                );
            }
            assert_eq!(
                (&actual.exit, &actual.stderr),
                (&expected.exit, &expected.stderr),
                "command {:?} drifted from the frozen capture",
                expected.argv
            );
            continue;
        }
        assert_eq!(
            (&actual.exit, &actual.stdout, &actual.stderr),
            (&expected.exit, &expected.stdout, &expected.stderr),
            "command {:?} drifted from the frozen capture",
            expected.argv
        );
    }

    // Independent of the fixture: the live catalog's own sequence invariants.
    let catalog: Value = serde_json::from_str(&captured[1].stdout).expect("live catalog JSON");
    let names = |key: &str| -> Vec<String> {
        catalog[key]
            .as_array()
            .unwrap_or_else(|| panic!("{key} must be an array"))
            .iter()
            .map(|entry| {
                entry
                    .as_str()
                    .or_else(|| entry["command"].as_str())
                    .expect("a command name")
                    .to_owned()
            })
            .collect()
    };
    let bound = names("commands");
    let unbound = names("commands_without_tools");
    let mut sorted = unbound.clone();
    sorted.sort();
    assert_eq!(unbound, sorted, "`commands_without_tools` must stay sorted");
    let mut unbound_unique = unbound.clone();
    unbound_unique.dedup();
    assert_eq!(
        unbound_unique.len(),
        unbound.len(),
        "`commands_without_tools` lists a command twice"
    );
    let mut bound_unique = bound.clone();
    bound_unique.sort();
    bound_unique.dedup();
    assert_eq!(
        bound_unique.len(),
        bound.len(),
        "`commands` lists a command twice"
    );
    for command in &bound {
        assert!(
            !unbound.contains(command),
            "`{command}` is both bound and unbound in the live catalog"
        );
    }
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
