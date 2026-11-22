// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module guards the committed CLI reference against the binary it
//! documents: it renders the whole manual with `clio help --markdown` and
//! compares every page's text with the committed copy.
//!
//! ## Owns
//!
//! - The render-and-compare of the whole manual against the committed
//!   reference under this crate's `tests/fixtures`
//! - The stale, missing, and extra page lines the failure reports
//!
//! ## Does not own
//!
//! - The manual's Markdown (see `cli_help_markdown`)
//! - The manual's write boundary (see `cli_manual_write`)
//! - Regenerating the committed copy: the failure prints the commands, and the
//!   reader runs them
//!
//! ## Boundary
//!
//! Tests here break when a command record changes the render without the
//! committed reference being regenerated. They never change the render, and
//! they never write the reference.

#[path = "../common/mod.rs"]
mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

/// The committed reference, relative to the repository root.
const REFERENCE: &str = "crates/clio/tests/fixtures/cli_reference";

/// The one command that regenerates the committed reference.
const REGENERATE: &str = "cargo run --locked -p clio --bin clio -- help --markdown \
                          --out crates/clio/tests/fixtures/cli_reference";

/// The `clio` binary with a cleared environment plus the coverage pattern.
fn clio() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_clio"));
    cmd.env_clear();
    // Forward the coverage profile pattern so instrumented children write into
    // the gitignored target directory instead of the source tree.
    if let Some(pattern) = std::env::var_os("LLVM_PROFILE_FILE") {
        cmd.env("LLVM_PROFILE_FILE", pattern);
    }
    common::seed_key(&mut cmd);
    cmd
}

/// The committed reference, inside this crate's own `tests` directory.
fn reference_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/cli_reference")
}

/// Calls served so far in this test binary, so a scratch directory never
/// repeats: the process id alone is constant across two runs of one scenario.
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Unique scratch directory for one case, removed when the guard drops.
fn temp_dir(tag: &str) -> common::Scratch {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    // Canonicalize the temp root before joining: the writer refuses every
    // symlink path component, and on macOS the system temp directory sits
    // behind symlinks, which would fail the write for an environmental reason.
    let base = std::fs::canonicalize(std::env::temp_dir()).expect("the temp root resolves");
    let dir = base.join(format!(
        "clio-cli-reference-{tag}-{}-{nanos}-{sequence}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    common::Scratch::adopt(dir)
}

/// Every Markdown page under `dir`, keyed by file name.
///
/// The map sorts by name, so a mismatch reports in the same order on every
/// machine and the directory's own iteration order never reaches an assertion.
fn pages(dir: &Path) -> BTreeMap<String, String> {
    let mut pages = BTreeMap::new();
    let entries =
        std::fs::read_dir(dir).unwrap_or_else(|error| panic!("read {}: {error}", dir.display()));
    for entry in entries {
        let path = entry.expect("directory entry").path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("md") {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .expect("a page name is UTF-8")
            .to_owned();
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        pages.insert(name, text);
    }
    pages
}

/// The failure lines for one render against the committed reference.
///
/// Every rendered page is checked first, so a stale page reports before an
/// extra one, and both loops walk a `BTreeMap`, so the order is the same on
/// every machine.
fn compare(
    rendered: &BTreeMap<String, String>,
    committed: &BTreeMap<String, String>,
) -> Vec<String> {
    let mut problems = Vec::new();
    for (name, text) in rendered {
        match committed.get(name) {
            None => problems.push(format!(
                "FAIL  {name} is missing from the committed reference"
            )),
            Some(golden) if golden != text => problems.push(format!(
                "FAIL  {name} is stale: it differs from the binary's render"
            )),
            Some(_) => {}
        }
    }
    for name in committed.keys() {
        if !rendered.contains_key(name) {
            problems.push(format!("FAIL  extra file {name} is not part of the manual"));
        }
    }
    problems
}

/// The commands that clear a failure, one removal per extra page.
///
/// Regeneration writes the pages the binary still has and leaves every other
/// file alone, so an extra page needs its own `rm`.
fn remedy(rendered: &BTreeMap<String, String>, committed: &BTreeMap<String, String>) -> String {
    let mut lines = vec![
        "Run from the repository root:".to_owned(),
        format!("  {REGENERATE}"),
    ];
    for name in committed.keys() {
        if !rendered.contains_key(name) {
            lines.push(format!("  rm {REFERENCE}/{name}"));
        }
    }
    lines.join("\n")
}

/// The committed reference is the binary's own render, page for page.
///
/// A command record that changes the manual without regenerating the reference
/// fails here, before the drift reaches a later commit.
#[test]
fn the_committed_reference_matches_the_binarys_render() {
    let out = temp_dir("render");
    let out_arg = out.to_str().expect("a UTF-8 scratch path");
    let output = clio()
        .args(["help", "--markdown", "--out", out_arg])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn clio");
    assert!(
        output.status.success(),
        "`clio help --markdown --out {out_arg}` failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let rendered = pages(&out);
    assert!(
        !rendered.is_empty(),
        "the render wrote no pages, so the comparison below proves nothing"
    );
    let committed = pages(&reference_dir());

    let problems = compare(&rendered, &committed);
    assert!(
        problems.is_empty(),
        "{}\n\n{}",
        problems.join("\n"),
        remedy(&rendered, &committed)
    );
}

/// One changed character in a page is reported as stale, and nothing else.
#[test]
fn a_changed_page_is_reported_as_stale() {
    let rendered = BTreeMap::from([("admit.md".to_owned(), "# Admit\n".to_owned())]);
    let committed = BTreeMap::from([("admit.md".to_owned(), "# Admit.\n".to_owned())]);
    assert_eq!(
        compare(&rendered, &committed),
        vec!["FAIL  admit.md is stale: it differs from the binary's render".to_owned()],
        "one changed character"
    );
}

/// A page the binary renders and the reference lacks is reported as missing.
#[test]
fn a_rendered_page_absent_from_the_reference_is_reported_as_missing() {
    let rendered = BTreeMap::from([("belief-count.md".to_owned(), "# Belief count\n".to_owned())]);
    let committed = BTreeMap::new();
    assert_eq!(
        compare(&rendered, &committed),
        vec!["FAIL  belief-count.md is missing from the committed reference".to_owned()]
    );
}

/// A page the reference has and the binary no longer renders is reported as
/// extra.
#[test]
fn a_committed_page_outside_the_render_is_reported_as_extra() {
    let rendered = BTreeMap::new();
    let committed = BTreeMap::from([("zz-extra.md".to_owned(), "# Gone\n".to_owned())]);
    assert_eq!(
        compare(&rendered, &committed),
        vec!["FAIL  extra file zz-extra.md is not part of the manual".to_owned()]
    );
}

/// Two equal maps report nothing, so a green run proves the comparison ran.
#[test]
fn identical_pages_report_nothing() {
    let rendered = BTreeMap::from([
        ("admit.md".to_owned(), "# Admit\n".to_owned()),
        ("batch.md".to_owned(), "# Batch\n".to_owned()),
    ]);
    let committed = rendered.clone();
    assert_eq!(compare(&rendered, &committed), Vec::<String>::new());
}

/// The remedy names the root, the regeneration, and one removal per extra page.
#[test]
fn the_remedy_removes_every_extra_page() {
    let rendered = BTreeMap::from([("admit.md".to_owned(), "# Admit\n".to_owned())]);
    let committed = BTreeMap::from([
        ("admit.md".to_owned(), "# Admit\n".to_owned()),
        ("zz-extra.md".to_owned(), "# Gone\n".to_owned()),
    ]);
    assert_eq!(
        remedy(&rendered, &committed),
        "Run from the repository root:\n  \
         cargo run --locked -p clio --bin clio -- help --markdown --out \
         crates/clio/tests/fixtures/cli_reference\n  \
         rm crates/clio/tests/fixtures/cli_reference/zz-extra.md"
    );
}

/// An identical pair needs no removal, so the remedy prints no `rm` line.
#[test]
fn the_remedy_for_an_identical_pair_prints_no_removal() {
    let rendered = BTreeMap::from([("admit.md".to_owned(), "# Admit\n".to_owned())]);
    let committed = rendered.clone();
    assert!(
        remedy(&rendered, &committed)
            .lines()
            .all(|line| !line.trim_start().starts_with("rm ")),
        "an identical pair needs no removal"
    );
}
