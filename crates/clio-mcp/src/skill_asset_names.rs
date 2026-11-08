// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module names the skill package assets: the router file at the package
//! root and one reference file per tool group, in catalog order.
//!
//! ## Owns
//! - The router file name and the reference directory name
//! - The reference file name of a group at one catalog index
//! - The expected asset set, and the check that a directory holds exactly it
//!
//! ## Does not own
//! - The group catalog itself (`clio_types::GROUP_TABLE`)
//! - Reading the directory or generating the embedded table (see `build.rs`)
//!
//! ## Boundary
//!
//! Changes to the asset naming convention belong here, and a build script and
//! the crate both read them from here, so the two can never disagree.
//!
//! Keep this module free of dependencies: the build script compiles it alone.

use std::collections::HashSet;

/// The skill router file, at the package root.
pub const SKILL_ROUTER_FILE: &str = "SKILL.md";

/// The directory inside the skill package that holds one reference per group.
pub const REFERENCES_DIR: &str = "references";

/// The reference file name for the group at `index` in the catalog order,
/// numbered so a directory listing reads in the order the router routes.
#[must_use]
pub fn reference_file_name(index: usize, group: &str) -> String {
    format!("{:02}-{group}.md", index + 1)
}

/// One reference file name per group, in catalog order.
#[must_use]
pub fn expected_reference_names(groups: &[&str]) -> Vec<String> {
    groups
        .iter()
        .enumerate()
        .map(|(index, group)| reference_file_name(index, group))
        .collect()
}

/// The whole expected asset set: the router file, then `references/<name>` for
/// every expected reference file.
#[must_use]
pub fn expected_asset_names(groups: &[&str]) -> Vec<String> {
    let mut names = vec![SKILL_ROUTER_FILE.to_owned()];
    names.extend(
        expected_reference_names(groups)
            .into_iter()
            .map(|name| format!("{REFERENCES_DIR}/{name}")),
    );
    names
}

/// Check that the reference files in the package are exactly the catalog's.
///
/// Every fault is reported at once, so one build names every wrong file instead
/// of one build per wrong file. The faults are sorted, deduplicated, and joined
/// with `; `.
///
/// # Errors
/// One message listing every expected file that is absent, every file that is
/// not in the catalog, and every file that appears more than once.
pub fn check_reference_set(groups: &[&str], found: &[String]) -> Result<(), String> {
    let expected = expected_reference_names(groups);
    let mut problems: Vec<String> = Vec::new();
    for name in &expected {
        if !found.contains(name) {
            problems.push(format!("missing reference file: {name}"));
        }
    }
    for name in found {
        if !expected.contains(name) {
            problems.push(format!("reference file is not in the catalog: {name}"));
        }
    }
    let mut seen: HashSet<&str> = HashSet::new();
    for name in found {
        if !seen.insert(name.as_str()) {
            problems.push(format!("duplicate reference file: {name}"));
        }
    }
    if problems.is_empty() {
        return Ok(());
    }
    problems.sort();
    problems.dedup();
    Err(problems.join("; "))
}

#[cfg(test)]
#[path = "skill_asset_names_tests.rs"]
mod skill_asset_names_tests;
