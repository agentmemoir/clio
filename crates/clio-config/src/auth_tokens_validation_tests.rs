// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for one workspace invariant: exactly one
//! constant-time token comparison exists, it lives here, and both transports
//! verify their presented secrets through it.
//!
//! ## Owns
//! - The workspace-wide scan for constant-time comparison definitions
//! - The refusal of every path that scan could not read or list
//! - The assertion that the shared helper's caveat about length leakage
//!   travels with its definition
//! - The assertion that the sync server and the MCP HTTP transport both call
//!   the shared helper on their auth paths
//!
//! ## Does not own
//! - The helper's behaviour coverage (`auth_tokens_tests`)
//! - The store's on-disk integrity (`auth_tokens_integrity_tests`)
//!
//! ## Boundary
//!
//! Tests here break when a second comparison is defined or a transport stops
//! calling the shared one; they never change the helper.
//!
//! Test-only. Never compiled into production builds.

use crate::sync_key_test_support::TempDir;
use std::fs;
use std::path::{Path, PathBuf};

/// One path the scan refused, what it refused to do, and the host's reason.
///
/// A refusal is returned rather than dropped: a guard that could not open a file
/// must say so, because "nothing is defined here" for a file it never read is how
/// a second comparison survives the guard meant to catch it.
type Refusal = (PathBuf, &'static str, String);

/// A source file the scan read: its path and its text.
type ReadFile = (PathBuf, String);

/// A directory the scan could not list.
const UNLISTABLE_DIR: &str = "a directory the scan could not list";

/// One entry of a listed directory that the scan could not read. The host's
/// error names no path, so the refusal is recorded against the parent directory.
const UNREADABLE_ENTRY: &str = "a directory entry the scan could not read";

/// A file the scan could not open or read to the end.
const UNREADABLE_FILE: &str = "a file the scan could not read";

/// Read one source file for the scan, or record why it refused.
///
/// This is the only place the scan opens a file, so a caller holding `None` has
/// read nothing and a refusal says so. A read error therefore never looks like a
/// file whose text happens to define nothing.
fn read_source(path: &Path, refusals: &mut Vec<Refusal>) -> Option<String> {
    match fs::read_to_string(path) {
        Ok(source) => Some(source),
        Err(err) => {
            refusals.push((path.to_path_buf(), UNREADABLE_FILE, err.to_string()));
            None
        }
    }
}

/// Every `.rs` source file under `root`, walked depth first and read, together
/// with every path the walk refused to read or list.
///
/// Files whose name ends in `_tests.rs`, `_test_support.rs`, `_fixtures.rs` or
/// `_seam.rs` are skipped: those four suffixes are the workspace's names for
/// test code, and a second comparison hidden in a test is not a transport's
/// auth path. Every other source file is scanned, including each binary
/// target, because a binary is a place a comparison could be defined.
///
/// The same walk is in `clio-types`'s date-arithmetic scan. Each crate keeps its
/// own copy rather than share public API, so a change to one goes in the other.
fn scan_sources(root: &Path) -> (Vec<ReadFile>, Vec<Refusal>) {
    let mut read = Vec::new();
    let mut refusals = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(err) => {
                refusals.push((dir.clone(), UNLISTABLE_DIR, err.to_string()));
                continue;
            }
        };
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(err) => {
                    // Defensive: provoking this needs a directory mutated between
                    // the listing and this read, so no test proves it fires.
                    refusals.push((dir.clone(), UNREADABLE_ENTRY, err.to_string()));
                    continue;
                }
            };
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext == "rs")
                && !is_test_file(&path)
                && let Some(source) = read_source(&path, &mut refusals)
            {
                read.push((path, source));
            }
        }
    }
    (read, refusals)
}

/// The same walk over the workspace's own crates directory, and the files it
/// read. This is the entry point the invariant tests use, so they do not have to
/// know where the workspace is. It asserts the refusals empty itself and returns
/// only the files, so no caller can report a half-scanned workspace as clean.
fn workspace_sources() -> Vec<ReadFile> {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../crates");
    let (read, refusals) = scan_sources(&crates);
    assert_no_refusals(&refusals);
    read
}

/// Assert the scan refused nothing, naming every path it could not read or
/// list. Checked once at the end of a scan, so a walk that stopped early fails
/// loudly instead of reporting a clean workspace.
fn assert_no_refusals(refusals: &[Refusal]) {
    assert!(
        refusals.is_empty(),
        "the scan must read every path it walks, but refused {} of them: {refusals:?}",
        refusals.len()
    );
}

/// Whether a source path is a test, fixture, or support file. Binary targets
/// are not test files and are scanned.
fn is_test_file(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    name.ends_with("_tests.rs")
        || name.ends_with("_test_support.rs")
        || name.ends_with("_fixtures.rs")
        || name.ends_with("_seam.rs")
}

/// The name a `fn` definition declares on one source line, if any.
fn defined_function(line: &str) -> Option<String> {
    let at = line.find("fn ")?;
    let rest = &line[at + 3..];
    let name: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    (!name.is_empty()).then_some(name)
}

/// Whether a name is a constant-time comparison: the shared helper's name or
/// any constant-time-named function a second implementation might use.
fn is_constant_time_compare(name: &str) -> bool {
    name == "ct_eq" || name.contains("constant_time")
}

/// The workspace defines exactly one constant-time token comparison, and it
/// is this module's `ct_eq`. A second definition — a copy, a rename, or a
/// hand-rolled compare under a new name — fails this test, so a reviewer who
/// adds one is told instead of inheriting two.
#[test]
fn the_workspace_defines_exactly_one_constant_time_comparison() {
    let read = workspace_sources();
    let mut definitions = Vec::new();
    for (file, source) in &read {
        for line in source.lines() {
            if let Some(name) = defined_function(line)
                && is_constant_time_compare(&name)
            {
                definitions.push((file.clone(), name));
            }
        }
    }
    assert_eq!(
        definitions.len(),
        1,
        "exactly one constant-time comparison is allowed, found {definitions:?}"
    );
    let (file, name) = &definitions[0];
    assert_eq!(name, "ct_eq", "the shared comparison is ct_eq");
    assert!(
        file.ends_with("clio-config/src/auth_tokens_validation.rs"),
        "the shared comparison lives in the shared token module, found {file:?}"
    );
}

/// A source file the scan cannot read is a refusal, not an absence. A dangling
/// symlink stands in for it: the directory lists it, so the walk reaches it, and
/// opening it fails, so it must leave the results and leave a refusal behind.
/// This one calls `scan_sources` rather than `workspace_sources`, because the
/// refusal it wants to see is the answer, not a failure.
#[test]
#[cfg(unix)]
fn the_scan_refuses_a_dangling_source_file_instead_of_dropping_it() {
    let dir = TempDir::new("dangling");
    let link = dir.path().join("dangling.rs");
    std::os::unix::fs::symlink(dir.path().join("gone.rs"), &link)
        .expect("the link to a missing file is created");

    let (read, refusals) = scan_sources(dir.path());

    assert!(
        !read.iter().any(|(path, _)| path == &link),
        "a file the scan could not read cannot join the results: {read:?}"
    );
    assert_eq!(refusals.len(), 1, "one unreadable file, found {refusals:?}");
    let (path, what, _) = &refusals[0];
    assert_eq!(path, &link, "the refusal names the file it could not read");
    assert_eq!(*what, UNREADABLE_FILE, "a file, not a directory, refused");
}

/// A root that is a regular file cannot be listed, and the whole subtree it was
/// to lead to is therefore unknown. The scan must record the root and return no
/// files, because returning no files alone reads as a directory with nothing in
/// it. It calls `scan_sources` directly, because here the refusal is the answer
/// rather than a failure.
#[test]
fn the_scan_refuses_a_root_that_is_a_file_rather_than_a_directory() {
    let dir = TempDir::new("root-is-a-file");
    let file = dir.path().join("not-a-directory.rs");
    fs::write(&file, "// not a directory\n").expect("the fixture file is written");

    let (files, refusals) = scan_sources(&file);

    assert!(
        files.is_empty(),
        "a root that is not a directory yields no files: {files:?}"
    );
    assert_eq!(refusals.len(), 1, "one unlistable root, found {refusals:?}");
    let (path, what, _) = &refusals[0];
    assert_eq!(path, &file, "the refusal names the root it could not list");
    assert_eq!(*what, UNLISTABLE_DIR, "a directory refused, not a file");
}

/// The helper's caveat travels with its definition: a length difference
/// returns early, which is acceptable for a fixed-length bearer token and not
/// for a variable-length secret. A future caller inherits the name with the
/// caveat, or this test fails.
#[test]
fn the_shared_comparison_carries_its_length_caveat() {
    let source = include_str!("auth_tokens_validation.rs");
    assert!(
        source.contains("a length difference returns early"),
        "{source}"
    );
    assert!(source.contains("fixed-length bearer token"), "{source}");
    assert!(source.contains("variable-length secret"), "{source}");
}

/// Both transports verify presented secrets through the shared helper: the
/// sync server's credential edge and the MCP HTTP transport's authorization.
#[test]
fn both_transports_call_the_shared_comparison() {
    let sync_edge = include_str!("../../clio-sync/src/credential.rs");
    let mcp_edge = include_str!("../../clio-mcp/src/http_auth.rs");
    assert!(
        sync_edge.matches("ct_eq(").count() >= 1,
        "the sync credential edge must compare through ct_eq"
    );
    assert!(
        mcp_edge.matches("ct_eq(").count() >= 1,
        "the MCP HTTP credential edge must compare through ct_eq"
    );
}
