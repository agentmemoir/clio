// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `clio compliance` surface: the group and subcommand
//! help screens, the exit codes, and the shape of the scope-inventory report
//! against injected streams, environment, and current directory.
//!
//! ## Owns
//! - The injected runner every test drives (`run_compliance`) and its scratch
//!   directory
//! - Exit-code and output-shape assertions for a missing store, an unreadable
//!   store, an unreachable Postgres target, repeated `--copy`, and usage
//!   refusals
//!
//! ## Does not own
//! - The command implementation under test (`compliance_cli`)
//! - The ledger's row semantics (see `clio-compliance`'s own tests) or the
//!   record's bytes (`cli_command_registry_compliance_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the surface's shape and exit codes.

use super::*;
use serde_json::{Value, json};

/// The nine export families the export manifest counts, in the order the
/// manifest declares them.
///
/// The scope ledger names many more families than the manifest counts, but it
/// always names these nine.
const EXPORT_FAMILIES: &[&str] = &[
    "items",
    "triples",
    "assoc_edges",
    "persona_stable",
    "persona_preferences",
    "tasks",
    "failures",
    "beliefs",
    "tombstones",
];

/// The report's serde field names, sorted.
const REPORT_FIELDS: &[&str] = &["backend", "complete", "rows", "unclassified"];

/// One row's serde field names, sorted.
const ROW_FIELDS: &[&str] = &[
    "custody",
    "family",
    "gap",
    "key_path",
    "open",
    "open_reason",
    "owner",
    "proof",
    "readability",
    "retention_action",
    "source",
    "subject_link",
];

/// The arguments as owned `String`s.
fn tokens(args: &[&str]) -> Vec<String> {
    args.iter().map(|arg| (*arg).to_owned()).collect()
}

/// An injected env lookup over fixed pairs; every unnamed key reads as absent,
/// so no test reaches the host environment.
fn env_from<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
    move |key: &str| {
        pairs
            .iter()
            .find(|(name, _)| *name == key)
            .map(|(_, value)| (*value).to_owned())
    }
}

/// Run one `compliance` invocation against injected streams, env, and cwd
/// (the injected-runner pattern of `backup_cli_fixtures`).
fn run_compliance(
    args: &[&str],
    env: &[(&str, &str)],
    current_dir: &Path,
    is_tty: bool,
) -> (ExitCode, String, String) {
    let lookup = env_from(env);
    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut input = std::io::Cursor::new(Vec::new());
    let code = {
        let mut io = CliIo {
            out: &mut out,
            err: &mut err,
            stdin: &mut input,
            is_tty,
            no_color: true,
            env: &lookup,
        };
        super::run(&mut io, tokens(args), Ok(current_dir.to_path_buf()))
    };
    (
        code,
        String::from_utf8(out).expect("utf8 out"),
        String::from_utf8(err).expect("utf8 err"),
    )
}

/// A unique scratch directory, removed on drop (the `backup_cli_fixtures`
/// scratch pattern, without its key-store setup: this surface reads no key).
struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock is set")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "clio-compliance-cli-{label}-{}-{nanos}",
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

/// Run `inventory` over a missing store and return the parsed JSON report.
fn missing_store_report(scratch: &Scratch) -> (ExitCode, Value, String) {
    let store = scratch.path().join("store.db");
    let (code, out, err) = run_compliance(
        &[
            "inventory",
            "--db",
            &store.display().to_string(),
            "--output",
            "json",
        ],
        &[],
        scratch.path(),
        false,
    );
    let payload = serde_json::from_str(out.trim()).unwrap_or(Value::Null);
    (code, payload, err)
}

/// A missing store is a report, not an error: exit 0, one compact JSON line,
/// `complete: false`, and at least one open row carrying a named reason.
#[test]
fn inventory_reports_a_missing_store_as_an_open_incomplete_report() {
    let scratch = Scratch::new("missing");
    let (code, payload, err) = missing_store_report(&scratch);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert!(
        payload.is_object(),
        "report must be a JSON object: {payload}"
    );
    assert_eq!(payload["backend"], json!("sqlite"), "{payload}");
    assert_eq!(payload["complete"], json!(false), "{payload}");
    let rows = payload["rows"].as_array().expect("rows array");
    assert!(!rows.is_empty(), "{payload}");
    assert!(
        rows.iter()
            .any(|row| row["open"] == json!(true) && row["open_reason"].is_string()),
        "a missing store must leave a named open row: {payload}"
    );
}

/// The report and every row serialize exactly their frozen field names.
#[test]
fn the_report_and_its_rows_carry_the_frozen_field_names() {
    let scratch = Scratch::new("fields");
    let (code, payload, err) = missing_store_report(&scratch);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let mut keys: Vec<&str> = payload
        .as_object()
        .expect("report object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(keys, REPORT_FIELDS, "report field names");
    for row in payload["rows"].as_array().expect("rows array") {
        let mut row_keys: Vec<&str> = row
            .as_object()
            .expect("row object")
            .keys()
            .map(String::as_str)
            .collect();
        row_keys.sort_unstable();
        assert_eq!(row_keys, ROW_FIELDS, "row {row}");
    }
}

/// The nine export families appear by name in every report, even when the
/// store is missing, so no family can silently drop out of the ledger.
#[test]
fn the_report_names_the_nine_export_families() {
    let scratch = Scratch::new("families");
    let (code, payload, err) = missing_store_report(&scratch);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let rows = payload["rows"].as_array().expect("rows array");
    for family in EXPORT_FAMILIES {
        assert!(
            rows.iter().any(|row| row["family"] == json!(family)),
            "missing family {family}: {payload}"
        );
    }
}

/// Two `--copy` occurrences declare two caller-held copies, both stay open,
/// and neither declared path is touched: the existing one keeps its exact
/// bytes through the run.
#[test]
fn repeated_copy_flags_declare_every_caller_copy_without_touching_it() {
    let scratch = Scratch::new("copies");
    let store = scratch.path().join("store.db");
    let held = scratch.path().join("held.jsonl");
    std::fs::write(&held, "{}\n").expect("write the held copy");
    let held_before = std::fs::read(&held).expect("read the held copy before the run");
    let gone = scratch.path().join("gone.jsonl");
    let (code, out, err) = run_compliance(
        &[
            "inventory",
            "--db",
            &store.display().to_string(),
            "--copy",
            &held.display().to_string(),
            "--copy",
            &gone.display().to_string(),
            "-o",
            "json",
        ],
        &[],
        scratch.path(),
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let payload: Value = serde_json::from_str(out.trim()).expect("json report");
    let declared: Vec<&Value> = payload["rows"]
        .as_array()
        .expect("rows array")
        .iter()
        .filter(|row| row["custody"] == json!("caller"))
        .collect();
    assert_eq!(declared.len(), 2, "one row per declared copy: {payload}");
    for row in &declared {
        assert_eq!(row["open"], json!(true), "{row}");
        assert!(row["open_reason"].is_string(), "{row}");
    }
    let held_after = std::fs::read(&held).expect("read the held copy after the run");
    assert_eq!(
        held_after, held_before,
        "the held copy's bytes must be unchanged by the run"
    );
}

/// The text view is one line per row plus the complete flag and the
/// unclassified list, and it names every export family.
#[test]
fn text_output_is_one_line_per_row_plus_the_summary_lines() {
    let scratch = Scratch::new("text");
    let (code, payload, err) = missing_store_report(&scratch);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let rows = payload["rows"].as_array().expect("rows array").len();
    let store = scratch.path().join("store.db");
    let (code, out, err) = run_compliance(
        &[
            "inventory",
            "--db",
            &store.display().to_string(),
            "--output",
            "text",
        ],
        &[],
        scratch.path(),
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let lines: Vec<&str> = out.trim().lines().collect();
    assert_eq!(lines.len(), rows + 2, "one line per row plus two: {out}");
    assert!(lines[rows].starts_with("complete: "), "{out}");
    assert!(lines[rows + 1].starts_with("unclassified: "), "{out}");
    for family in EXPORT_FAMILIES {
        assert!(out.contains(family), "missing family {family}: {out}");
    }
}

/// An unreadable store (a directory where the file belongs) is a structured
/// operational failure, never an empty complete inventory: exit 1 and a JSON
/// envelope naming what could not be read.
#[test]
fn an_unreadable_store_is_a_structured_error() {
    let scratch = Scratch::new("unreadable");
    let store = scratch.path().join("not-a-file.db");
    std::fs::create_dir(&store).expect("create the directory in the store's place");
    let (code, out, err) = run_compliance(
        &[
            "inventory",
            "--db",
            &store.display().to_string(),
            "-o",
            "json",
        ],
        &[],
        scratch.path(),
        false,
    );
    assert_eq!(code, ExitCode::from(crate::cli_error::EXIT_OPERATIONAL));
    let payload: Value = serde_json::from_str(err.trim()).expect("structured error envelope");
    assert_eq!(payload["ok"], json!(false), "{payload}");
    let message = payload["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("not a regular file") && message.contains("not-a-file.db"),
        "the error must name the unreadable path: {message}"
    );
    assert!(
        out.trim().is_empty(),
        "nothing is reported on stdout: {out}"
    );
}

/// An unreachable Postgres target is the same class of structured failure:
/// exit 1 with an envelope, never a report.
#[test]
fn an_unreachable_postgres_target_is_a_structured_error() {
    let scratch = Scratch::new("postgres");
    let (code, out, err) = run_compliance(
        &[
            "inventory",
            "--backend",
            "postgres",
            "--db",
            "postgres://127.0.0.1:1/clio",
            "-o",
            "json",
        ],
        &[],
        scratch.path(),
        false,
    );
    assert_eq!(code, ExitCode::from(crate::cli_error::EXIT_OPERATIONAL));
    let payload: Value = serde_json::from_str(err.trim()).expect("structured error envelope");
    assert_eq!(payload["ok"], json!(false), "{payload}");
    assert!(
        payload["message"]
            .as_str()
            .is_some_and(|text| !text.is_empty()),
        "{payload}"
    );
    assert!(
        out.trim().is_empty(),
        "nothing is reported on stdout: {out}"
    );
}

/// `sqlite::memory:` is refused (exit 2): a fresh process has no on-disk store
/// to inventory, and reporting an empty one as complete would be a lie.
#[test]
fn the_in_memory_store_is_refused() {
    let scratch = Scratch::new("memory");
    let (code, out, err) = run_compliance(
        &["inventory", "--db", "sqlite::memory:"],
        &[],
        scratch.path(),
        false,
    );
    assert_eq!(code, ExitCode::from(crate::cli_error::EXIT_USAGE));
    assert!(err.contains("sqlite::memory:"), "{err}");
    assert!(out.trim().is_empty(), "{out}");
}

/// An unknown subcommand is refused with a hint, an unknown flag fails closed,
/// an extra positional is refused, and a bad `--output` value is refused: each
/// exits 2 and writes nothing to stdout.
#[test]
fn usage_refusals_exit_two() {
    let scratch = Scratch::new("usage");
    let store = scratch.path().join("store.db");
    let store = store.display().to_string();
    for args in [
        vec!["inventorys"],
        vec!["inventory", "--db", store.as_str(), "--copyz", "x"],
        vec!["inventory", "--db", store.as_str(), "extra"],
        vec!["inventory", "--db", store.as_str(), "--output", "yaml"],
        vec!["inventory", "--db", store.as_str(), "--backend", "mysql"],
    ] {
        let (code, out, err) = run_compliance(&args, &[], scratch.path(), false);
        assert_eq!(
            code,
            ExitCode::from(crate::cli_error::EXIT_USAGE),
            "{args:?}: {err}"
        );
        assert!(out.trim().is_empty(), "{args:?} wrote stdout: {out}");
        assert!(!err.trim().is_empty(), "{args:?} wrote no error");
    }
}

/// `clio compliance <help spelling>` and `clio compliance inventory --help`
/// print the registered screens and exit 0, without a store.
#[test]
fn the_help_screens_print_the_registered_text() {
    let scratch = Scratch::new("help");
    for args in [vec!["--help"], vec!["help"]] {
        let (code, out, err) = run_compliance(&args, &[], scratch.path(), false);
        assert_eq!(code, ExitCode::SUCCESS, "{err}");
        assert!(
            out.contains("clio compliance inventory [--output json|text]"),
            "{out}"
        );
        assert!(out.contains("Ask a subcommand for its flags"), "{out}");
    }
    let (code, out, err) = run_compliance(&["inventory", "--help"], &[], scratch.path(), false);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert!(
        out.contains("clio compliance inventory [--output json|text]"),
        "{out}"
    );
    let note = crate::cli_command_registry::find("compliance inventory")
        .expect("the record is registered")
        .notes[0];
    assert!(out.contains(note), "{out}");
}
