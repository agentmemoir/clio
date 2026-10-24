// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the pure per-item helpers the import planner
//! needs: the id a bundle row is written under, the admission candidate
//! discriminator, and the masking pass applied to an item before it is staged.
//!
//! ## Owns
//! - `import_id`: the write id for a row, re-keyed when the bundle targets
//!   another bank
//! - `kind_key`: the admission kind/key pair for a gated item create
//! - `masked_item`: the masking pass on a staged item clone
//!
//! ## Does not own
//! - Planning order, dedupe, and gating (`import_apply.rs`)
//! - The secret-shape rules themselves (`clio-config` scrubbing, `export_mask`)
//!
//! ## Boundary
//!
//! Every function here is pure and reads no planner state. Changes to how an id
//! is derived, which value is masked, or what admission is keyed on should
//! normally require changes here.
//!
//! Keep this module focused on those pure helpers.

use std::borrow::Cow;

use clio_config::secret::{masked_clone, scrub_inline_secrets};
use clio_types::{ItemKind, MemoryItem, SemanticCategory};

use crate::export_mask::scrub_context;

/// `items.id`-style primary keys are deployment-unique, so a cross-bank
/// restore writes a derived id; derived from (source id, target bank) only, so
/// re-importing the same bundle into the same bank stays idempotent.
pub(crate) fn import_id<'i>(bank: &str, source_bank: &str, id: &'i str) -> Cow<'i, str> {
    if bank == source_bank {
        Cow::Borrowed(id)
    } else {
        Cow::Owned(format!("{id}#{bank}"))
    }
}

/// Candidate discriminator strings for the gated create path.
pub(crate) fn kind_key(item: &MemoryItem) -> (&'static str, &'static str) {
    match item.kind {
        ItemKind::Semantic => (
            "semantic",
            item.category.map_or("", SemanticCategory::as_str),
        ),
        ItemKind::Episodic => (
            "episodic",
            item.episodic_type
                .map_or("", clio_types::EpisodicType::as_str),
        ),
    }
}

/// Defensive masking pass before a write (diagnostics-grade masking on top of
/// the export-time masking already present in the bundle). A context that
/// scrubs to an empty string is treated as absent, never written as `Some("")`.
pub(crate) fn masked_item(item: &MemoryItem) -> MemoryItem {
    let mut out = item.clone();
    if let Some(snapshot) = out.snapshot.as_mut() {
        *snapshot = masked_clone(snapshot);
    }
    if let Some(gist) = out.gist.as_mut() {
        *gist = scrub_inline_secrets(gist);
    }
    out.context = scrub_context(out.context.as_deref());
    out
}

#[cfg(test)]
#[path = "import_apply_keys_tests.rs"]
mod tests;
