// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the embedded skill assets: one router file plus exactly one
//! reference file per group in the catalog, every one of them non-empty.
//!
//! ## Owns
//! - Assertions over the size, order, and text of `SKILL_ASSETS`
//! - The match between the embedded reference names and the catalog order
//!
//! ## Does not own
//! - The naming and checking rules themselves (see `skill_asset_names_tests`)
//!
//! ## Boundary
//!
//! Changes to which assets are embedded should normally require changes here.
//!
//! Keep this module focused on the embedded table.

use super::{SKILL_ASSETS, references};
use crate::skill_asset_names::{REFERENCES_DIR, reference_file_name};

#[test]
fn the_table_holds_the_router_file_and_one_reference_per_group() {
    assert_eq!(SKILL_ASSETS.len(), clio_types::GROUP_TABLE.len() + 1);
    assert_eq!(SKILL_ASSETS[0].0, "SKILL.md");
    assert!(
        !SKILL_ASSETS[0].1.trim().is_empty(),
        "the router file text is embedded"
    );
}

#[test]
fn every_reference_is_non_empty_and_in_catalog_order() {
    let refs = references();
    assert_eq!(refs.len(), clio_types::GROUP_TABLE.len());
    for (index, (name, text)) in refs.iter().enumerate() {
        let group = clio_types::GROUP_TABLE[index].group.as_str();
        assert_eq!(
            *name,
            format!("{REFERENCES_DIR}/{}", reference_file_name(index, group))
        );
        assert!(!text.trim().is_empty(), "{name} has text: it is embedded");
    }
}
