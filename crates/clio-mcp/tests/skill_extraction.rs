// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for proving that the skill package assets are in
//! the built test binary and nowhere else: it extracts every embedded asset
//! into a directory it creates and compares the bytes it reads back.
//!
//! ## Owns
//! - `the_built_binary_extracts_every_embedded_asset_without_a_source_tree`
//! - The child mode that writes `SKILL_ASSETS` into the directory named by
//!   `CLIO_SKILL_ASSET_EXTRACT_DIR`
//!
//! ## Does not own
//! - The embedded table (`clio_mcp::skill_assets`)
//! - The asset names and the expected set (`clio_mcp::skill_asset_names`)
//! - Which groups the catalog holds (`clio_types::GROUP_TABLE`)
//!
//! ## Boundary
//!
//! The test re-execs its own binary and reads back only the directory it made
//! under the system temporary directory, so it opens no repository path and no
//! socket: the bytes it compares are the ones the compiler put in the binary it
//! is running. A change to how the assets are embedded belongs to `build.rs`; a
//! change to which assets are expected belongs to the group catalog. The one
//! variable is set on the child's `Command` only, so the parent environment and
//! every peer test are untouched.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::UNIX_EPOCH;

use clio_mcp::skill_asset_names::expected_asset_names;
use clio_mcp::skill_assets::SKILL_ASSETS;
use clio_types::GROUP_TABLE;

/// The test this file holds: the parent re-execs the binary with this name, and
/// the child returns as soon as it has written the assets.
const TEST_NAME: &str = "the_built_binary_extracts_every_embedded_asset_without_a_source_tree";

/// The variable that turns the one test into the child that extracts the assets.
const EXTRACT_VAR: &str = "CLIO_SKILL_ASSET_EXTRACT_DIR";

/// The directory the child fills, under the temporary root the parent owns.
const ASSETS_DIR: &str = "assets";

/// A temporary root removed when it goes out of scope, on every exit path.
struct TempRoot(PathBuf);

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Every catalog group id, in catalog order: the expected asset names derive
/// from this one list, so no check keeps a hand-written copy of the catalog.
fn groups() -> Vec<&'static str> {
    GROUP_TABLE.iter().map(|spec| spec.group.as_str()).collect()
}

/// A root no other test run shares: the process id and the current nanosecond.
fn new_root() -> PathBuf {
    let nanos = match UNIX_EPOCH.elapsed() {
        Ok(since) => since.as_nanos(),
        Err(_) => 0,
    };
    std::env::temp_dir().join(format!("clio-skill-assets-{}-{nanos}", std::process::id()))
}

/// Write every embedded asset under `root`, creating the directories it needs.
fn extract(root: &Path) {
    for (name, text) in SKILL_ASSETS {
        let path = root.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .unwrap_or_else(|error| panic!("cannot create `{}`: {error}", parent.display()));
        }
        fs::write(&path, text)
            .unwrap_or_else(|error| panic!("cannot write `{}`: {error}", path.display()));
    }
}

/// Every file under `root`, named relative to it, top level and one level deep.
fn files_under(root: &Path) -> Vec<String> {
    let mut names = Vec::new();
    let entries = fs::read_dir(root)
        .unwrap_or_else(|error| panic!("cannot read `{}`: {error}", root.display()));
    for entry in entries {
        let entry = entry.unwrap_or_else(|error| {
            panic!("cannot read an entry of `{}`: {error}", root.display())
        });
        let name = entry.file_name().into_string().unwrap_or_else(|raw| {
            panic!(
                "`{}` holds `{}`, a name that is not UTF-8",
                root.display(),
                raw.display()
            )
        });
        if entry.path().is_dir() {
            let inner = fs::read_dir(entry.path()).unwrap_or_else(|error| {
                panic!("cannot read `{}`: {error}", entry.path().display())
            });
            for child in inner {
                let child = child.unwrap_or_else(|error| {
                    panic!(
                        "cannot read an entry of `{}`: {error}",
                        entry.path().display()
                    )
                });
                let leaf = child.file_name().into_string().unwrap_or_else(|raw| {
                    panic!(
                        "`{}` holds `{}`, a name that is not UTF-8",
                        entry.path().display(),
                        raw.display()
                    )
                });
                names.push(format!("{name}/{leaf}"));
            }
        } else {
            names.push(name);
        }
    }
    names.sort_unstable();
    names
}

#[test]
fn the_built_binary_extracts_every_embedded_asset_without_a_source_tree() {
    if let Some(destination) = std::env::var_os(EXTRACT_VAR) {
        extract(Path::new(&destination));
        return;
    }

    let root = TempRoot(new_root());
    fs::create_dir_all(&root.0).unwrap_or_else(|error| {
        panic!(
            "cannot create the temporary root `{}`: {error}",
            root.0.display()
        )
    });
    let assets = root.0.join(ASSETS_DIR);
    let output = Command::new(std::env::current_exe().expect("this test binary has a path"))
        .args(["--exact", TEST_NAME])
        .current_dir(&root.0)
        .env(EXTRACT_VAR, &assets)
        .output()
        .unwrap_or_else(|error| panic!("cannot run the extraction child: {error}"));
    assert!(
        output.status.success(),
        "the extraction child exited with {}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(
        assets.join("SKILL.md").is_file(),
        "the child wrote no `SKILL.md` under `{}`",
        assets.display()
    );
    assert!(
        assets.join("references").is_dir(),
        "the child wrote no `references` directory under `{}`",
        assets.display()
    );

    let mut expected = expected_asset_names(&groups());
    let count = expected.len();
    assert_eq!(
        SKILL_ASSETS.len(),
        count,
        "the binary embeds {} assets against the {count} the catalog names: embed one asset per catalog group plus the router file",
        SKILL_ASSETS.len()
    );
    expected.sort_unstable();
    let written = files_under(&assets);
    assert_eq!(
        written, expected,
        "the extracted set is not the set the catalog names: extract every embedded asset, because an asset the catalog names and the binary lacks is guidance a caller never receives"
    );

    for (name, text) in SKILL_ASSETS {
        let path = assets.join(name);
        let bytes = fs::read(&path)
            .unwrap_or_else(|error| panic!("cannot read `{}`: {error}", path.display()));
        assert!(
            bytes == text.as_bytes(),
            "the extracted `{name}` differs from the embedded text: write the embedded text unchanged, because the bytes a caller reads are the ones the binary carries"
        );
    }
}
