// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This build script embeds the skill package assets into the crate: it writes
//! one table of the router file and every group reference file, read from
//! `skills/clio-mcp` and named from the validated group catalog.
//!
//! ## Owns
//! - Reading the reference directory and checking it against the catalog
//! - Generating the `SKILL_ASSETS` table into `OUT_DIR`
//! - Failing the build on a missing expected file, an orphan file, or a duplicate
//!
//! ## Does not own
//! - The asset names themselves (see `skill_asset_names`, shared with the crate)
//! - The catalog the names come from (`clio_types::GROUP_TABLE`)
//!
//! ## Boundary
//!
//! Changes to how the assets are embedded belong here. Changes to which assets
//! are expected belong in the catalog, not in this script.
//!
//! The script never keeps a hand-written list: an asset the catalog does not
//! name, or a file the catalog names that the package lacks, fails the build.

use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

#[path = "src/skill_asset_names.rs"]
mod skill_asset_names;

use skill_asset_names::{
    REFERENCES_DIR, SKILL_ROUTER_FILE, check_reference_set, expected_asset_names,
};

fn main() {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR")
        .expect("the build script runs under cargo, which sets CARGO_MANIFEST_DIR");
    let out_dir =
        env::var("OUT_DIR").expect("the build script runs under cargo, which sets OUT_DIR");
    let package_dir = fs::canonicalize(PathBuf::from(manifest_dir).join("../../skills/clio-mcp"))
        .unwrap_or_else(|error| panic!("cannot find the skill package directory: {error}"));
    let references_dir = package_dir.join(REFERENCES_DIR);
    let router = package_dir.join(SKILL_ROUTER_FILE);
    assert!(
        router.is_file(),
        "the skill router file is missing: {}",
        router.display()
    );

    let found = file_names(&references_dir);
    let groups: Vec<&str> = clio_types::GROUP_TABLE
        .iter()
        .map(|spec| spec.group.as_str())
        .collect();
    if let Err(message) = check_reference_set(&groups, &found) {
        panic!("the skill package assets do not match the group catalog: {message}");
    }

    let mut generated = String::new();
    push_header(&mut generated);
    for name in expected_asset_names(&groups) {
        let path = package_dir.join(&name);
        push_asset(&mut generated, &name, &path);
    }
    generated.push_str("];\n");

    let generated_path = Path::new(&out_dir).join("skill_assets.rs");
    fs::write(&generated_path, generated).unwrap_or_else(|error| {
        let shown = generated_path.display();
        panic!("cannot write the embedded skill assets to {shown}: {error}")
    });
    println!("cargo::rerun-if-changed={}", package_dir.display());
}

/// The names of the files in `directory`, or a panic when one cannot be read or
/// is not valid UTF-8.
fn file_names(directory: &Path) -> Vec<String> {
    let entries = fs::read_dir(directory).unwrap_or_else(|error| {
        let shown = directory.display();
        panic!("cannot read the skill reference directory {shown}: {error}")
    });
    entries
        .map(|entry| {
            let entry = entry.unwrap_or_else(|error| {
                let shown = directory.display();
                panic!("cannot read an entry of the skill reference directory {shown}: {error}")
            });
            entry.file_name().into_string().unwrap_or_else(|name| {
                let shown = directory.display();
                let name = name.to_string_lossy();
                panic!("a file name in the skill reference directory {shown} is not valid UTF-8: {name}")
            })
        })
        .collect()
}

/// The doc line and the opening of the generated table.
fn push_header(generated: &mut String) {
    generated.push_str("/// The skill package assets embedded in this build: the router file\n");
    generated.push_str("/// first, then one reference file per group in catalog order.\n");
    generated.push_str("pub const SKILL_ASSETS: &[(&str, &str)] = &[\n");
}

/// One row of the generated table: the asset name and the file embedded by path.
fn push_asset(generated: &mut String, name: &str, path: &Path) {
    writeln!(
        generated,
        "    (\"{name}\", include_str!(r#\"{}\"#)),",
        path.display()
    )
    .expect("writing to a String cannot fail");
}
