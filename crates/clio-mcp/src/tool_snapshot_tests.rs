// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for tests of one immutable tool-view snapshot:
//! its accessors and the digest inputs a cursor binds to.
//!
//! ## Owns
//! - A stable digest for equal inputs and a different digest for a changed one
//! - Read access to the view and the tools a snapshot carries
//!
//! ## Does not own
//! - Snapshot storage or replacement (see `tool_signals`)
//! - Cursor encoding (see `tools_page`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::ToolSnapshot;
use crate::tools_page::ToolView;
use clio_types::{ToolGroup, ToolPreset};
use serde_json::json;

fn view(groups: Vec<ToolGroup>, revision: &str) -> ToolView {
    ToolView {
        preset: Some(ToolPreset::All),
        groups,
        revision: revision.to_owned(),
    }
}

#[test]
fn equal_inputs_produce_the_same_digest() {
    let tools = vec![json!({ "name": "search" })];
    let first = ToolSnapshot::build(view(vec![ToolGroup::Retrieval], "1"), tools.clone());
    let second = ToolSnapshot::build(view(vec![ToolGroup::Retrieval], "1"), tools);
    assert_eq!(first.digest(), second.digest());
}

#[test]
fn a_changed_view_or_tool_list_changes_the_digest() {
    let tools = vec![json!({ "name": "search" })];
    let base = ToolSnapshot::build(view(vec![ToolGroup::Retrieval], "1"), tools.clone());
    let groups = ToolSnapshot::build(view(vec![ToolGroup::ItemWrite], "1"), tools.clone());
    let revision = ToolSnapshot::build(view(vec![ToolGroup::Retrieval], "2"), tools);
    let catalog = ToolSnapshot::build(
        view(vec![ToolGroup::Retrieval], "1"),
        vec![json!({ "name": "store" })],
    );
    for changed in [&groups, &revision, &catalog] {
        assert_ne!(base.digest(), changed.digest());
    }
}

#[test]
fn the_snapshot_exposes_its_view_and_tools() {
    let tools = vec![json!({ "name": "search" })];
    let snapshot = ToolSnapshot::build(view(vec![ToolGroup::Retrieval], "1"), tools.clone());
    assert_eq!(snapshot.tools(), tools.as_slice());
    assert_eq!(snapshot.view().groups, vec![ToolGroup::Retrieval]);
    assert_ne!(snapshot.digest(), "");
}
