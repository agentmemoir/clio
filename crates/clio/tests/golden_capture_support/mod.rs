// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module holds the test-only golden-capture tooling for the CLI's help
//! surfaces: the fixture records, the cargo freshness guard, the capture gate,
//! the capture plan, the baseline comparator, and the two helpers a test needs
//! to run a pinned child and to write a scratch file. No tests live here.
//!
//! ## Owns
//! - The capture gate, the freshness guards, the capture plan, and the
//!   baseline comparator
//! - The public re-exports of its child modules
//!
//! ## Does not own
//! - Any assertion (`golden_capture_test`, `golden_repeatability_test`)
//! - The fixture records and their writer (`fixture`)
//! - The pinned child environment, the bounded run, and one capture (`child`)
//! - The scratch directory a file-writing test uses (`scratch`)
//! - The help text and catalog themselves (`cli_help`, the binary's `print_help`)
//!
//! ## Boundary
//!
//! Changes to how captures are taken, guarded, or compared should normally
//! require changes here. Changes to what the help output says, and to the
//! fixture file's shape, should not.
//!
//! Keep this module focused on golden-capture plumbing.

// Capture-mode items run in the capture tool, not in this test binary.
#![allow(dead_code)]

use serde_json::Value;
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

mod child;
mod fixture;
mod scratch;

use child::run_bounded;

// Compiled into two test targets; each uses a different slice of the re-exports.
#[allow(unused_imports)]
pub use child::{capture, pin_capture_env};
#[allow(unused_imports)]
pub use fixture::{
    CaptureEntry, FIXTURE_VERSION, GoldenFixture, fixture_path, load_fixture, write_fixture,
};
#[allow(unused_imports)]
pub use scratch::Scratch;

/// Set on the capture run's environment to turn capture mode on.
pub const CAPTURE_ENV: &str = "CLIO_GOLDEN_CAPTURE";
/// Pinned onto every captured child so its output never names the host's home.
/// [`child::pin_capture_env`] is what holds the rest of the child's identity.
pub const CAPTURE_HOME: &str = "/home/clio";

/// Generous bounds: a hang safety net, never a synchronization mechanism.
const CHILD_DEADLINE: Duration = Duration::from_secs(30);
const BUILD_DEADLINE: Duration = Duration::from_secs(600);

/// Why the capture guard refused to freeze anything. Each case names one guard.
#[derive(Clone, Debug, PartialEq)]
pub enum GuardRefusal {
    BuildFailed { exit: i32 },
    StaleArtifact,
    NoFreshnessProof,
    MissingExecutable,
    SourceMutated,
}

impl std::fmt::Display for GuardRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            Self::BuildFailed { exit } => {
                format!("capture refused: the release build failed with exit {exit}")
            }
            Self::StaleArtifact => String::from(
                "capture refused: the cache was prepared but cargo recompiled clio, \
                 so re-run the capture",
            ),
            Self::NoFreshnessProof => String::from(
                "capture refused: the build gave no freshness proof for the clio package",
            ),
            Self::MissingExecutable => {
                String::from("capture refused: the release executable is missing")
            }
            Self::SourceMutated => String::from("capture refused: source changed while capturing"),
        };
        f.write_str(&text)
    }
}

/// Read cargo's JSON compiler-artifact stream and judge freshness: the clio
/// bin artifact's `executable` is returned only when its `fresh` flag proves
/// cargo reused the prepared cache. Lines that do not parse as JSON are
/// skipped; a rebuilt artifact or a missing proof refuses.
pub fn freshness_verdict(cargo_exit: i32, cargo_stdout: &str) -> Result<String, GuardRefusal> {
    if cargo_exit != 0 {
        return Err(GuardRefusal::BuildFailed { exit: cargo_exit });
    }
    let artifact = cargo_stdout
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|message| {
            message["reason"] == "compiler-artifact"
                && message["target"]["name"] == "clio"
                && message["target"]["kind"]
                    .as_array()
                    .is_some_and(|kinds| kinds.iter().any(|kind| kind == "bin"))
        });
    let Some(artifact) = artifact else {
        return Err(GuardRefusal::NoFreshnessProof);
    };
    match artifact["fresh"].as_bool() {
        Some(false) => Err(GuardRefusal::StaleArtifact),
        Some(true) => match artifact["executable"].as_str() {
            Some(path) if !path.is_empty() => Ok(path.to_owned()),
            _ => Err(GuardRefusal::NoFreshnessProof),
        },
        None => Err(GuardRefusal::NoFreshnessProof),
    }
}

/// The artifact's executable path must exist on disk, so a capture never
/// freezes output from a binary that was moved or removed after the build.
pub fn require_executable(path: &Path) -> Result<(), GuardRefusal> {
    if path.exists() {
        return Ok(());
    }
    Err(GuardRefusal::MissingExecutable)
}

/// Pure source-identity check: identical text is not a mutation.
#[must_use]
pub fn source_mutated(before: &str, after: &str) -> bool {
    before != after
}

/// The source identity of `root`: the git head, plus a hash of the dirty
/// `git status --porcelain` text when the tree is dirty. None when git fails.
pub fn source_identity(root: &Path) -> Option<String> {
    let head = git_stdout(root, ["rev-parse", "HEAD"])?;
    let status = git_stdout(root, ["status", "--porcelain"])?;
    if status.trim().is_empty() {
        return Some(head);
    }
    let mut hasher = DefaultHasher::new();
    status.hash(&mut hasher);
    Some(format!("{}-dirty-{:016x}", head, hasher.finish()))
}

/// One git probe's trimmed stdout, or None when git fails or exits non-zero.
fn git_stdout(root: &Path, args: [&str; 2]) -> Option<String> {
    let mut command = Command::new("git");
    command.arg("-C").arg(root).args(args);
    let output = run_bounded(&mut command, CHILD_DEADLINE).ok()?;
    output.code.filter(|code| *code == 0)?;
    String::from_utf8(output.stdout)
        .ok()
        .map(|text| text.trim().to_owned())
}

/// One difference between the baseline and a fresh capture, naming the exact
/// command and the changed output; missing and extra commands go by position.
#[derive(Clone, Debug, PartialEq)]
pub enum ContentDiff {
    MissingCommand {
        position: usize,
    },
    UnexpectedCommand {
        argv: Vec<String>,
    },
    FieldMismatch {
        argv: Vec<String>,
        field: String,
        expected: String,
        actual: String,
    },
}

impl ContentDiff {
    /// A plain report of this difference. It never suggests rewriting the baseline.
    #[must_use]
    pub fn report(&self) -> String {
        match self {
            Self::MissingCommand { position } => {
                format!("baseline command at position {position} was not captured")
            }
            Self::UnexpectedCommand { argv } => {
                format!("captured command {argv:?} has no baseline entry")
            }
            Self::FieldMismatch {
                argv,
                field,
                expected,
                actual,
            } => format!(
                "command {argv:?} changed its {field}: expected {expected:?}, got {actual:?}"
            ),
        }
    }
}

/// Compare a fresh capture against the baseline fixture, order-sensitive:
/// each position's argv identity is checked before its exit code and streams.
/// Returns every difference; an empty vector means the commands and their
/// bytes are identical.
#[must_use]
pub fn compare(fixture: &GoldenFixture, captured: &[CaptureEntry]) -> Vec<ContentDiff> {
    let exit_of = |code: Option<i32>| code.map_or_else(|| "signal".into(), |exit| exit.to_string());
    let mut diffs = Vec::new();
    let shared = fixture.entries.len().min(captured.len());
    for (position, expected) in fixture.entries.iter().enumerate().take(shared) {
        let actual = &captured[position];
        if expected.argv != actual.argv {
            diffs.push(ContentDiff::FieldMismatch {
                argv: expected.argv.clone(),
                field: "argv".to_owned(),
                expected: format!("{:?}", expected.argv),
                actual: format!("{:?}", actual.argv),
            });
        }
        let fields = [
            ("exit", exit_of(expected.exit), exit_of(actual.exit)),
            ("stdout", expected.stdout.clone(), actual.stdout.clone()),
            ("stderr", expected.stderr.clone(), actual.stderr.clone()),
        ];
        for (field, want, got) in fields {
            if want != got {
                diffs.push(ContentDiff::FieldMismatch {
                    argv: expected.argv.clone(),
                    field: field.to_owned(),
                    expected: want,
                    actual: got,
                });
            }
        }
    }
    for position in shared..fixture.entries.len() {
        diffs.push(ContentDiff::MissingCommand { position });
    }
    for entry in &captured[shared..] {
        diffs.push(ContentDiff::UnexpectedCommand {
            argv: entry.argv.clone(),
        });
    }
    diffs
}

/// The catalog never lists the `auth token` verbs, so the capture plan
/// appends them after the catalog leaves, in registry order.
pub(crate) static EXTRA_LEAVES: &[&str] =
    &["auth token add", "auth token list", "auth token revoke"];

/// The capture plan from the running binary itself: top-level help and the
/// JSON catalog first, then one `["<cmd>", "--help"]` per leaf command in
/// catalog order plus the auth-token extras. No duplicates.
pub fn capture_plan(binary: &Path) -> Vec<Vec<String>> {
    let help = capture(binary, &["help".to_owned()], None).expect("top-level help capture");
    let catalog =
        capture(binary, &["help".to_owned(), "--json".to_owned()], None).expect("catalog capture");
    let mut plan = vec![help.argv, catalog.argv];
    let parsed: Value = serde_json::from_str(&catalog.stdout).expect("catalog stdout is JSON");
    for name in leaf_commands(&parsed) {
        let mut argv: Vec<String> = name.split(' ').map(str::to_owned).collect();
        argv.push("--help".to_owned());
        plan.push(argv);
    }
    for name in EXTRA_LEAVES {
        let mut argv: Vec<String> = name.split(' ').map(str::to_owned).collect();
        argv.push("--help".to_owned());
        if !plan.contains(&argv) {
            plan.push(argv);
        }
    }
    plan
}

/// Every leaf command in the catalog: `commands` in binding-table order, then
/// `commands_without_tools` sorted, deduped, minus group prefixes — names
/// another name extends.
fn leaf_commands(catalog: &Value) -> Vec<String> {
    static NO_ENTRIES: [Value; 0] = [];
    let array = |key: &str| -> &[Value] {
        match catalog.get(key).and_then(Value::as_array) {
            Some(commands) => commands,
            None => &NO_ENTRIES,
        }
    };
    let mut names: Vec<String> = Vec::new();
    for entry in array("commands") {
        if let Some(command) = entry.get("command").and_then(Value::as_str) {
            push_unique(&mut names, command.to_owned());
        }
    }
    for name in array("commands_without_tools") {
        if let Some(name) = name.as_str() {
            push_unique(&mut names, name.to_owned());
        }
    }
    let mut prefixes: Vec<String> = Vec::new();
    for name in &names {
        let prefix = format!("{name} ");
        if names.iter().any(|other| other.starts_with(&prefix)) {
            prefixes.push(name.clone());
        }
    }
    names.retain(|name| !prefixes.contains(name));
    names
}

fn push_unique(names: &mut Vec<String>, name: String) {
    if !names.contains(&name) {
        names.push(name);
    }
}

/// The last `count` non-empty lines of `text`, for a bounded failure report.
fn last_lines(text: &str, count: usize) -> String {
    let lines: Vec<&str> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    let start = lines.len().saturating_sub(count);
    lines[start..].join("\n")
}

/// First ancestor of `start` whose Cargo.toml holds a `[workspace]` section, or
/// None when no ancestor qualifies (an unreadable manifest never matches).
pub fn workspace_root(start: &Path) -> Option<PathBuf> {
    let mut current = Some(start);
    while let Some(dir) = current {
        let manifest = fs::read_to_string(dir.join("Cargo.toml"));
        if manifest.is_ok_and(|text| text.lines().any(|l| l.trim() == "[workspace]")) {
            return Some(dir.to_path_buf());
        }
        current = dir.parent();
    }
    None
}

/// Run `cargo build -p clio --release --locked --message-format=json` in the
/// workspace root above `root`, accept the clio bin artifact's executable only
/// with its freshness proof, verify the path exists, and return it; every
/// refusal names its guard.
pub fn guarded_build(root: &Path) -> Result<String, String> {
    let workspace = workspace_root(root)
        .ok_or_else(|| "capture refused: no workspace root found above the root".to_owned())?;
    let mut command = Command::new("cargo");
    command.current_dir(&workspace).args([
        "build",
        "-p",
        "clio",
        "--release",
        "--locked",
        "--message-format=json",
    ]);
    let output = run_bounded(&mut command, BUILD_DEADLINE)
        .map_err(|error| format!("the release build did not finish: {error}"))?;
    let stdout = String::from_utf8(output.stdout)
        .map_err(|error| format!("cargo build stdout is not UTF-8: {error}"))?;
    let stderr = String::from_utf8(output.stderr)
        .map_err(|error| format!("cargo build stderr is not UTF-8: {error}"))?;
    let exit = output.code.unwrap_or(-1);
    let executable = freshness_verdict(exit, &stdout).map_err(|refusal| match refusal {
        GuardRefusal::BuildFailed { .. } => format!(
            "capture refused: the release build failed with exit {exit}\ncargo stderr: {}",
            last_lines(&stderr, 5)
        ),
        refusal => refusal.to_string(),
    })?;
    require_executable(Path::new(&executable)).map_err(|refusal| refusal.to_string())?;
    Ok(executable)
}
