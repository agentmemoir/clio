// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for one immutable tool-view snapshot: the view
//! identity, the visible tools, and the digest a cursor binds to.
//!
//! ## Owns
//! - The snapshot value every paging and listening caller shares
//! - The canonical digest over the catalog hash, the preset, the groups, and
//!   the revision
//! - Read access to the view, the tools, and the digest
//!
//! ## Does not own
//! - Reading the live configuration into a snapshot (see `tools_page`)
//! - Storing or swapping the installed snapshot (see `tool_signals`)
//! - Cursor encoding or page slicing (see `tools_page`)
//!
//! ## Boundary
//!
//! Changes to what a snapshot holds, or to what its digest covers, should
//! normally require changes here. Changes to how a snapshot is published
//! should not.
//!
//! Keep this module focused on one immutable view value.

use crate::schema::{catalog_hash, fnv1a64_hex};
use crate::tools_page::ToolView;
use clio_types::ToolPreset;
use serde_json::Value;
use std::sync::Arc;

/// The digest word for a view an installed selection governs, so it cannot be
/// read as one of the preset names.
const INSTALLED_SELECTION: &str = "installed";

/// One immutable tool view, shared by every caller until it is replaced.
#[derive(Debug)]
pub(crate) struct ToolSnapshot {
    /// The preset, group selection, and revision the view was built under.
    #[cfg_attr(not(test), allow(dead_code))]
    view: ToolView,
    /// The visible definitions, discovery entries first.
    tools: Arc<Vec<Value>>,
    /// The canonical digest over the view and the tools.
    digest: String,
}

impl ToolSnapshot {
    /// Build one snapshot and its digest.
    pub(crate) fn build(view: ToolView, tools: Vec<Value>) -> Self {
        let digest = digest_of(&tools, &view);
        Self {
            view,
            tools: Arc::new(tools),
            digest,
        }
    }

    /// The view identity this snapshot was built under.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn view(&self) -> &ToolView {
        &self.view
    }

    /// The visible definitions this snapshot advertises.
    pub(crate) fn tools(&self) -> &[Value] {
        &self.tools
    }

    /// The digest a cursor binds to.
    pub(crate) fn digest(&self) -> &str {
        &self.digest
    }

    /// A snapshot with a forced digest, for tests that drive the change signal.
    #[cfg(test)]
    pub(crate) fn with_digest(digest: &str) -> Self {
        Self {
            view: ToolView {
                preset: None,
                groups: Vec::new(),
                revision: String::new(),
            },
            tools: Arc::new(Vec::new()),
            digest: digest.to_owned(),
        }
    }
}

/// The digest of the view a cursor belongs to.
///
/// It covers the hash of the visible definitions, the preset name when one
/// applies, the effective group selection, and the configuration revision, so a
/// cursor stops working when any of them changes.
fn digest_of(tools: &[Value], view: &ToolView) -> String {
    let mut canonical = String::new();
    canonical.push_str(&catalog_hash(tools));
    canonical.push('\n');
    canonical.push_str(view.preset.map_or(INSTALLED_SELECTION, ToolPreset::as_str));
    canonical.push('\n');
    for group in &view.groups {
        canonical.push_str(group.as_str());
        canonical.push('\n');
    }
    canonical.push_str(&view.revision);
    fnv1a64_hex(canonical.as_bytes())
}

#[cfg(test)]
#[path = "tool_snapshot_tests.rs"]
mod tool_snapshot_tests;
