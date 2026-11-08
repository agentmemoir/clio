// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the skill asset names: the numbering and order of the
//! reference files, and the check that refuses a missing, an orphan, or a
//! duplicated reference file by name.
//!
//! ## Owns
//! - Assertions over `reference_file_name` and the expected name lists
//! - The missing, orphan, and duplicate refusals of `check_reference_set`
//!
//! ## Does not own
//! - The embedded asset table built from these names (see `skill_assets_tests`)
//!
//! ## Boundary
//!
//! Changes to the naming convention or to the refusal messages should normally
//! require changes here.
//!
//! Keep this module focused on the asset names.

use super::{
    REFERENCES_DIR, SKILL_ROUTER_FILE, check_reference_set, expected_asset_names,
    expected_reference_names, reference_file_name,
};

/// A short catalog standing in for the real one, in the real catalog's order.
const GROUPS: &[&str] = &["retrieval", "item-write", "item-read"];

/// The message of a refused set, or a panic when the set was accepted.
fn refusal(groups: &[&str], found: &[String]) -> String {
    check_reference_set(groups, found).expect_err("the check should refuse this set")
}

#[test]
fn a_reference_is_numbered_from_one_and_named_by_its_group() {
    assert_eq!(reference_file_name(0, "retrieval"), "01-retrieval.md");
    assert_eq!(reference_file_name(9, "graph"), "10-graph.md");
    assert_eq!(reference_file_name(16, "compliance"), "17-compliance.md");
}

#[test]
fn the_expected_names_keep_the_catalog_order() {
    assert_eq!(
        expected_reference_names(GROUPS),
        vec!["01-retrieval.md", "02-item-write.md", "03-item-read.md"]
    );
}

#[test]
fn the_expected_assets_start_with_the_router_file() {
    let names = expected_asset_names(GROUPS);
    assert_eq!(names.len(), GROUPS.len() + 1);
    assert_eq!(names[0], SKILL_ROUTER_FILE);
    assert_eq!(
        names[1],
        format!("{REFERENCES_DIR}/01-retrieval.md"),
        "the second asset is the first reference"
    );
}

#[test]
fn the_complete_set_is_accepted() {
    let found = expected_reference_names(GROUPS);
    assert_eq!(check_reference_set(GROUPS, &found), Ok(()));
}

#[test]
fn a_missing_reference_is_refused_by_name() {
    let found: Vec<String> = expected_reference_names(GROUPS)
        .into_iter()
        .filter(|name| name != "02-item-write.md")
        .collect();
    let message = refusal(GROUPS, &found);
    assert!(
        message.contains("missing reference file: 02-item-write.md"),
        "the message names the missing file: {message}"
    );
}

#[test]
fn an_orphan_reference_is_refused_by_name() {
    let mut found = expected_reference_names(GROUPS);
    found.push("99-graph.md".to_owned());
    let message = refusal(GROUPS, &found);
    assert!(
        message.contains("reference file is not in the catalog: 99-graph.md"),
        "the message names the orphan file: {message}"
    );
}

#[test]
fn a_duplicate_reference_is_refused_by_name() {
    let mut found = expected_reference_names(GROUPS);
    found.push("01-retrieval.md".to_owned());
    let message = refusal(GROUPS, &found);
    assert!(
        message.contains("duplicate reference file: 01-retrieval.md"),
        "the message names the duplicated file: {message}"
    );
}

#[test]
fn every_fault_is_reported_in_one_message() {
    let found = vec!["01-retrieval.md".to_owned(), "99-graph.md".to_owned()];
    let message = refusal(GROUPS, &found);
    assert!(
        message.contains("02-item-write.md") && message.contains("03-item-read.md"),
        "both missing files are named: {message}"
    );
    assert!(
        message.contains("99-graph.md"),
        "the orphan file is named: {message}"
    );
}
