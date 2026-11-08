// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for proving that every skill package asset is in
//! the built test binary: it writes every embedded asset into a directory it
//! creates and compares the bytes it reads back with the text it embeds.
//!
//! ## Owns
//! - `the_built_binary_extracts_every_embedded_asset_without_a_source_tree`
//! - The temporary directory that write fills and the guard that removes it
//!
//! ## Does not own
//! - The embedded table (`clio_mcp::skill_assets`)
//! - The asset names and the expected set (`clio_mcp::skill_asset_names`)
//! - Which groups the catalog holds (`clio_types::GROUP_TABLE`)
//!
//! ## Boundary
//!
//! The test reads only the contents of `SKILL_ASSETS` and the directory it made
//! under the system temporary directory, so it opens no repository path and no
//! socket: the bytes it compares are the ones the compiler put in the binary it
//! is running. A change to how the assets are embedded belongs to `build.rs`; a
//! change to which assets are expected belongs to the group catalog. The
//! directory is always removed, so a failing run leaves nothing behind.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use clio_mcp::skill_asset_names::{REFERENCES_DIR, SKILL_ROUTER_FILE, expected_asset_names};
use clio_mcp::skill_assets::SKILL_ASSETS;
use clio_types::GROUP_TABLE;

/// The directory the test fills, under the temporary root it owns.
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

/// A root under the system temporary directory, named by this process and the
/// current nanosecond so a leftover from an earlier run is not reused.
fn new_root() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    std::env::temp_dir().join(format!("clio-skill-assets-{}-{nanos}", std::process::id()))
}

/// Write every embedded asset under `assets`, creating the directories it needs.
fn extract(assets: &Path) {
    for (name, text) in SKILL_ASSETS {
        let path = assets.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .unwrap_or_else(|error| panic!("cannot create `{}`: {error}", parent.display()));
        }
        fs::write(&path, text)
            .unwrap_or_else(|error| panic!("cannot write `{}`: {error}", path.display()));
    }
}

#[test]
fn the_built_binary_extracts_every_embedded_asset_without_a_source_tree() {
    let root = TempRoot(new_root());
    let assets = root.0.join(ASSETS_DIR);
    fs::create_dir_all(&assets).unwrap_or_else(|error| {
        panic!(
            "cannot create the temporary directory `{}`: {error}",
            assets.display()
        )
    });
    extract(&assets);

    let router = assets.join(SKILL_ROUTER_FILE);
    assert!(
        router.is_file(),
        "the binary embeds no `{SKILL_ROUTER_FILE}`: embed the router file so a caller loads the skill body it ships"
    );
    let references = assets.join(REFERENCES_DIR);
    assert!(
        references.is_dir(),
        "the binary embeds no `{REFERENCES_DIR}` directory: embed one reference file per catalog group"
    );

    let expected = expected_asset_names(&groups());
    let count = expected.len();
    assert_eq!(
        SKILL_ASSETS.len(),
        count,
        "the binary embeds {} assets against the {count} the catalog names: embed one asset per catalog group plus the router file",
        SKILL_ASSETS.len()
    );

    let embedded: BTreeMap<&str, &str> = SKILL_ASSETS.iter().copied().collect();
    for name in expected {
        let text = embedded.get(name.as_str()).unwrap_or_else(|| {
            panic!(
                "the catalog names the asset `{name}` and the binary does not embed it: embed one asset per catalog group plus the router file"
            )
        });
        let path = assets.join(&name);
        let bytes = fs::read(&path)
            .unwrap_or_else(|error| panic!("cannot read `{}`: {error}", path.display()));
        assert!(
            bytes == text.as_bytes(),
            "the extracted `{name}` differs from the embedded text: write the embedded text unchanged, because the bytes a caller reads are the ones the binary carries"
        );
    }
}
