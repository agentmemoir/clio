// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module proves the golden-capture tooling's own behavior: the freshness
//! verdict on injected cargo output, the comparator's diffs and its refusal to
//! touch the baseline file, source-mutation detection, fixture round-trips,
//! and a bounded real-binary capture and catalog-derived capture plan.
//!
//! ## Owns
//! - Every assertion over the golden-capture support module
//!
//! ## Does not own
//! - The capture tooling itself (`golden_capture_support`, and the scratch
//!   directory guard it holds)
//! - The help output the captures freeze (`cli_help`, the binary's `print_help`)
//!
//! ## Boundary
//!
//! Changes to what is asserted about capture and comparison should normally
//! require changes here. Changes to how captures are bounded or how fixtures
//! are written should not.
//!
//! Keep this module focused on the golden-capture contract.

use serde_json::Value;
use std::fs;
use std::path::Path;
use std::process::Command;

#[path = "golden_capture_support/mod.rs"]
mod golden_capture_support;

use golden_capture_support::{
    CAPTURE_HOME, CaptureEntry, ContentDiff, FIXTURE_VERSION, GoldenFixture, GuardRefusal, Scratch,
    capture, capture_plan, compare, freshness_verdict, load_fixture, pin_capture_env,
    require_executable, source_mutated, workspace_root, write_fixture,
};

/// Injected `cargo build -p clio --release --locked --message-format=json`
/// stdout: the clio bin artifact, fresh, with cargo's executable path.
const FRESH_BUILD_JSON: &str = concat!(
    "{\"reason\":\"compiler-artifact\",\"target\":{\"name\":\"clio\",\"kind\":[\"bin\"]},",
    "\"fresh\":true,\"executable\":\"/repo/target/release/clio\"}\n",
);

fn entry(argv: &[&str], exit: Option<i32>, stdout: &str, stderr: &str) -> CaptureEntry {
    CaptureEntry {
        argv: argv.iter().map(|token| (*token).to_owned()).collect(),
        exit,
        stdout: stdout.to_owned(),
        stderr: stderr.to_owned(),
    }
}

fn fixture_with(entries: Vec<CaptureEntry>) -> GoldenFixture {
    GoldenFixture {
        version: FIXTURE_VERSION,
        entries,
    }
}

/// Fresh JSON — cargo reused the prepared cache — returns the artifact's executable.
#[test]
fn fresh_clio_json_verdict_returns_the_artifact_executable() {
    let verdict = freshness_verdict(0, FRESH_BUILD_JSON);
    assert_eq!(verdict, Ok("/repo/target/release/clio".to_owned()));
}

/// A `fresh: false` artifact refuses as `StaleArtifact`, naming why in its Display.
#[test]
fn rebuilt_clio_json_refuses_as_stale_artifact() {
    let stale = FRESH_BUILD_JSON.replace("\"fresh\":true", "\"fresh\":false");
    let refusal = freshness_verdict(0, &stale).expect_err("stale build must refuse");
    assert_eq!(refusal, GuardRefusal::StaleArtifact);
    let text = refusal.to_string();
    assert!(text.contains("re-run the capture"), "{text}");
}

/// Stdout without a fresh clio bin artifact refuses as `NoFreshnessProof`: empty
/// output, non-JSON lines, a lib-only artifact, and a null executable.
#[test]
fn json_without_a_fresh_bin_artifact_refuses_as_no_freshness_proof() {
    let no_proof = |label: &str, stdout: &str| {
        let refusal = freshness_verdict(0, stdout).expect_err("must refuse");
        assert_eq!(refusal, GuardRefusal::NoFreshnessProof, "{label}");
        assert!(refusal.to_string().contains("freshness proof"), "{label}");
    };
    no_proof("empty stdout", "");
    no_proof("non-JSON lines", "not json at all\n");
    let lib_only = FRESH_BUILD_JSON.replace("\"kind\":[\"bin\"]", "\"kind\":[\"lib\"]");
    no_proof("lib artifact", &lib_only);
    let null_path = FRESH_BUILD_JSON.replace("\"/repo/target/release/clio\"", "null");
    no_proof("null executable", &null_path);
}

/// A non-zero cargo exit refuses as `BuildFailed` with the exit carried through.
#[test]
fn failed_cargo_exit_refuses_as_build_failed() {
    let refusal = freshness_verdict(2, FRESH_BUILD_JSON).expect_err("failed build must refuse");
    assert_eq!(refusal, GuardRefusal::BuildFailed { exit: 2 });
    assert!(refusal.to_string().contains("build failed"), "{refusal}");
}

/// A missing path refuses as `MissingExecutable`; an existing scratch file is accepted.
#[test]
fn require_executable_refuses_a_missing_path_and_accepts_an_existing_one() {
    let scratch = Scratch::new("require-executable");
    let missing = scratch.0.join("missing").join("clio");
    let refusal = require_executable(&missing).expect_err("a missing path must refuse");
    assert_eq!(refusal, GuardRefusal::MissingExecutable);
    let text = refusal.to_string();
    assert!(text.contains("release executable is missing"), "{text}");
    let file = scratch.0.join("clio");
    fs::write(&file, b"binary").expect("write the scratch file");
    assert_eq!(require_executable(&file), Ok(()));
}

/// Identical source identity is not a mutation; any text change is.
#[test]
fn source_mutation_is_exactly_a_text_change() {
    assert!(
        !source_mutated("abc", "abc"),
        "identical text is not a mutation"
    );
    assert!(source_mutated("abc", "abd"), "changed text is a mutation");
}

/// An identical capture produces no difference at all.
#[test]
fn identical_capture_yields_no_differences() {
    let baseline = fixture_with(vec![entry(&["help"], Some(0), "usage text", "")]);
    let captured = vec![entry(&["help"], Some(0), "usage text", "")];
    let diffs = compare(&baseline, &captured);
    assert!(diffs.is_empty(), "identical output must not diff");
}

/// A renamed command with identical bytes reports one argv mismatch; a matching one reports none.
#[test]
fn renamed_command_with_identical_bytes_reports_only_an_argv_mismatch() {
    let baseline = fixture_with(vec![entry(&["help"], Some(0), "usage text", "")]);
    let renamed = vec![entry(&["--help"], Some(0), "usage text", "")];
    let diffs = compare(&baseline, &renamed);
    assert_eq!(diffs.len(), 1, "only argv differs: {diffs:?}");
    assert!(
        matches!(&diffs[0], ContentDiff::FieldMismatch { field, .. } if field == "argv"),
        "{diffs:?}"
    );
    let same = vec![entry(&["help"], Some(0), "usage text", "")];
    let matching = compare(&baseline, &same);
    assert!(matching.is_empty(), "matching argv and bytes");
}

/// One changed stdout byte yields exactly one `FieldMismatch` naming the command and field.
#[test]
fn changed_stdout_byte_reports_one_field_mismatch_on_that_command() {
    let baseline = fixture_with(vec![entry(&["help"], Some(0), "usage text", "")]);
    let captured = vec![entry(&["help"], Some(0), "usage text!", "")];
    let diffs = compare(&baseline, &captured);
    assert_eq!(diffs.len(), 1, "one changed byte yields one diff");
    assert!(matches!(
        &diffs[0],
        ContentDiff::FieldMismatch { field, expected, actual, .. }
            if field == "stdout" && expected == "usage text" && actual == "usage text!"
    ));
    let report = diffs[0].report();
    assert!(report.contains("[\"help\"]"), "{report}");
}

/// A changed stderr stream yields a `FieldMismatch` on the stderr field.
#[test]
fn changed_stderr_reports_a_field_mismatch_on_stderr() {
    let baseline = fixture_with(vec![entry(&["help"], Some(0), "out", "err")]);
    let captured = vec![entry(&["help"], Some(0), "out", "error")];
    let diffs = compare(&baseline, &captured);
    assert!(
        matches!(
            diffs.first(),
            Some(ContentDiff::FieldMismatch { field, .. }) if field == "stderr"
        ),
        "{diffs:?}"
    );
}

/// A changed exit code yields a `FieldMismatch` on the exit field, carrying both codes.
#[test]
fn changed_exit_reports_a_field_mismatch_on_exit() {
    let baseline = fixture_with(vec![entry(&["help"], Some(0), "out", "")]);
    let captured = vec![entry(&["help"], Some(1), "out", "")];
    let diffs = compare(&baseline, &captured);
    assert!(
        matches!(
            diffs.first(),
            Some(ContentDiff::FieldMismatch { field, expected, actual, .. })
                if field == "exit" && expected == "0" && actual == "1"
        ),
        "{diffs:?}"
    );
}

/// A shorter capture reports `MissingCommand`; a longer one `UnexpectedCommand`.
#[test]
fn missing_and_extra_commands_report_by_position() {
    let baseline = fixture_with(vec![
        entry(&["help"], Some(0), "out", ""),
        entry(&["help", "--json"], Some(0), "{}", ""),
    ]);
    let shorter = vec![entry(&["help"], Some(0), "out", "")];
    let missing = vec![ContentDiff::MissingCommand { position: 1 }];
    assert_eq!(compare(&baseline, &shorter), missing);
    let longer = vec![
        entry(&["help"], Some(0), "out", ""),
        entry(&["help", "--json"], Some(0), "{}", ""),
        entry(&["version"], Some(0), "clio 0.1.0", ""),
    ];
    let extra = ContentDiff::UnexpectedCommand {
        argv: vec!["version".to_owned()],
    };
    assert_eq!(compare(&baseline, &longer), vec![extra]);
}

/// Comparing against a tampered capture fails and leaves the baseline file byte-identical.
#[test]
fn comparator_fails_a_changed_command_without_touching_the_baseline_file() {
    let scratch = Scratch::new("comparator-no-rewrite");
    let path = scratch.0.join("baseline.json");
    let baseline = fixture_with(vec![entry(&["help"], Some(0), "usage text", "")]);
    write_fixture(&baseline, &path).expect("write the baseline fixture");
    let before = fs::read(&path).expect("read the baseline before comparing");
    let loaded = load_fixture(&path).expect("load the baseline fixture");
    let captured = vec![entry(&["help"], Some(0), "changed text", "")];
    let diffs = compare(&loaded, &captured);
    assert_eq!(diffs.len(), 1, "tampered capture must differ: {diffs:?}");
    let after = fs::read(&path).expect("read the baseline after comparing");
    assert_eq!(before, after, "compare must not rewrite the baseline file");
}

/// Capturing the real binary's help records exit 0 and its argv, bare and COLUMNS-pinned.
#[test]
fn bounded_help_capture_records_exit_zero_and_streams() {
    let binary = Path::new(env!("CARGO_BIN_EXE_clio"));
    let bare = capture(binary, &["help".to_owned()], None).expect("help capture without COLUMNS");
    assert_eq!(bare.exit, Some(0), "help must exit 0: {}", bare.stderr);
    assert!(!bare.stdout.is_empty(), "help stdout must not be empty");
    assert_eq!(bare.argv, vec!["help".to_owned()]);
    let pinned =
        capture(binary, &["help".to_owned()], Some("100")).expect("help capture with COLUMNS=100");
    assert_eq!(pinned.exit, Some(0), "help must exit 0: {}", pinned.stderr);
    assert!(!pinned.stdout.is_empty(), "help stdout must not be empty");
}

/// The plan starts with top-level help and the catalog, then holds exactly one
/// `["<cmd>", "--help"]` per catalog leaf, derived independently, with no duplicates.
#[test]
fn capture_plan_lists_help_and_catalog_then_one_help_per_catalog_leaf() {
    static NO_ENTRIES: [Value; 0] = [];
    let binary = Path::new(env!("CARGO_BIN_EXE_clio"));
    let plan = capture_plan(binary);
    assert_eq!(plan[0], vec!["help".to_owned()], "top-level help first");
    let catalog_argv = vec!["help".to_owned(), "--json".to_owned()];
    assert_eq!(plan[1], catalog_argv, "then the catalog");
    let catalog = capture(binary, &catalog_argv, None).expect("catalog capture");
    let catalog: Value = serde_json::from_str(&catalog.stdout).expect("catalog stdout is JSON");
    let array = |key: &str| -> &[Value] {
        match catalog.get(key).and_then(Value::as_array) {
            Some(commands) => commands,
            None => &NO_ENTRIES,
        }
    };
    let mut names: Vec<String> = Vec::new();
    for bound in array("commands") {
        if let Some(command) = bound.get("command").and_then(Value::as_str) {
            names.push(command.to_owned());
        }
    }
    for unbound in array("commands_without_tools") {
        if let Some(name) = unbound.as_str() {
            names.push(name.to_owned());
        }
    }
    // An independent traversal: a leaf is a catalog name no other name extends.
    let leaves: Vec<Vec<String>> = names
        .iter()
        .filter(|name| {
            !names
                .iter()
                .any(|other| other.starts_with(&format!("{name} ")))
        })
        .map(|name| {
            let mut argv: Vec<String> = name.split(' ').map(str::to_owned).collect();
            argv.push("--help".to_owned());
            argv
        })
        .collect();
    assert_eq!(
        &plan[2..],
        leaves.as_slice(),
        "plan leaves must match the catalog"
    );
    let mut unique = leaves.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        leaves.len(),
        "no duplicate leaves: {leaves:?}"
    );
}

/// The workspace walk-up returns the nearest ancestor whose Cargo.toml declares `[workspace]`.
#[test]
fn workspace_root_finds_the_workspace_manifest_above_nested_dirs() {
    let scratch = Scratch::new("workspace-root");
    fs::write(scratch.0.join("Cargo.toml"), "[workspace]\n").expect("write the workspace manifest");
    fs::create_dir_all(scratch.0.join("a/b")).expect("create the nested tree");
    let nested = scratch.0.join("a/b");
    assert_eq!(
        workspace_root(&nested),
        Some(scratch.0.clone()),
        "the walk-up must stop at the workspace manifest"
    );
}

/// A plain `[package]` manifest does not stop the walk-up, so no ancestor yields None.
#[test]
fn workspace_root_without_a_workspace_manifest_returns_none() {
    let scratch = Scratch::new("workspace-root-none");
    fs::write(scratch.0.join("Cargo.toml"), "[package]\n").expect("write a plain manifest");
    fs::create_dir_all(scratch.0.join("a/b")).expect("create the nested tree");
    let nested = scratch.0.join("a/b");
    assert_eq!(
        workspace_root(&nested),
        None,
        "a package manifest must not satisfy the workspace guard"
    );
}

/// A fixture written to scratch and read back equals the original.
#[test]
fn fixture_roundtrip_through_a_file_preserves_the_entries() {
    let scratch = Scratch::new("roundtrip");
    let path = scratch.0.join("golden.json");
    let fixture = fixture_with(vec![
        entry(&["help"], Some(0), "usage text", ""),
        entry(&["help", "--json"], Some(0), "{\"commands\":[]}", ""),
    ]);
    write_fixture(&fixture, &path).expect("write the fixture");
    assert_eq!(
        load_fixture(&path).expect("load the fixture back"),
        fixture,
        "the roundtrip must preserve the fixture"
    );
}

/// A written fixture stays within the 450-line cap, one compact `{"argv":` line per entry.
#[test]
fn written_fixture_stays_within_the_line_cap_one_line_per_entry() {
    let scratch = Scratch::new("compact-fixture");
    let path = scratch.0.join("golden.json");
    let fixture = fixture_with(vec![
        entry(&["help"], Some(0), "usage text\nwith two lines\n", ""),
        entry(&["help", "--json"], Some(0), "{\"commands\":[]}", "warn\n"),
    ]);
    write_fixture(&fixture, &path).expect("write the fixture");
    let text = String::from_utf8(fs::read(&path).expect("read the fixture back"))
        .expect("the fixture is UTF-8");
    let lines: Vec<&str> = text.lines().collect();
    assert!(
        lines.len() <= 450,
        "written fixture too long: {}",
        lines.len()
    );
    let entry_lines = lines
        .iter()
        .filter(|line| line.starts_with("{\"argv\":"))
        .count();
    assert_eq!(
        entry_lines,
        fixture.entries.len(),
        "one compact line per entry: {text}"
    );
}

/// A capture pins the child's HOME to `CAPTURE_HOME`: the caller's home never
/// appears in the captured output, and an embedded default db path starts
/// with the pinned home, so the bytes are identical on every host.
#[test]
fn captured_help_output_pins_home_and_never_names_the_caller_home() {
    let binary = Path::new(env!("CARGO_BIN_EXE_clio"));
    let captured = capture(binary, &["help".to_owned()], None).expect("help capture");
    let home = std::env::var("HOME").expect("the caller has a HOME");
    assert!(
        !captured.stdout.contains(&home) && !captured.stderr.contains(&home),
        "captured output must not name the caller's home: {}",
        captured.stdout
    );
    let path = captured
        .stdout
        .split_whitespace()
        .find(|token| token.ends_with("clio.db"))
        .expect("the help embeds the default db path");
    assert!(
        path.starts_with(CAPTURE_HOME),
        "the embedded default db path must start with the pinned home: {path}"
    );
}

/// A deployment config on the capture machine cannot reach a capture: the
/// pinned child loads none at all, while the unpinned control beside it does,
/// so this test fails if the pin stops disabling the load.
#[test]
fn a_deployment_config_on_the_capture_machine_cannot_reach_a_capture() {
    let home = Scratch::new("pinned-home-config");
    let dir = home.0.join(".config/clio");
    fs::create_dir_all(&dir).expect("create the config directory");
    fs::write(dir.join("deployment.json"), "{\"backend\":\"postgres\"}\n")
        .expect("write the deployment config");
    let binary = Path::new(env!("CARGO_BIN_EXE_clio"));
    let pinned = help_default_line(binary, &home.0, true);
    let unpinned = help_default_line(binary, &home.0, false);
    assert!(
        pinned.ends_with("(backend: sqlite)"),
        "the pinned child must read no config file: {pinned}"
    );
    assert!(
        unpinned.ends_with("(backend: postgres)"),
        "the control child must read the config file: {unpinned}"
    );
}

/// The `default:` line of one `help` child under `home`, pinned when asked.
fn help_default_line(binary: &Path, home: &Path, pinned: bool) -> String {
    let mut command = Command::new(binary);
    if pinned {
        pin_capture_env(&mut command, home, None);
    } else {
        command.env_clear();
        command.env("HOME", home);
    }
    let output = command.args(["help"]).output().expect("run help");
    let stdout = String::from_utf8(output.stdout).expect("help writes UTF-8");
    stdout
        .lines()
        .find(|line| line.contains("default:"))
        .expect("the help prints the default database")
        .to_owned()
}
