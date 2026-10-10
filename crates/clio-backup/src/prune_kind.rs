// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for one question about a backup listing: which of
//! the listed entries are symbolic links rather than real files.
//!
//! ## Owns
//! - The one `symlink_metadata` per listed entry, read without following a link
//! - The `LinkedPaths` answer that prune selection is given as data
//!
//! ## Does not own
//! - Choosing the keep set and the candidates (see `prune`)
//! - Refusing to follow a link at delete time (see `prune_delete`)
//! - Listing the backup directory and reading manifests (see `inventory`)
//!
//! ## Boundary
//!
//! Changes to how a listed entry is classified should normally require changes
//! here. Changes to which archives a prune keeps should not require changes here.
//!
//! Keep this module focused on the link question, and metadata-only: it opens no
//! archive and reads no manifest.

use crate::inventory::SnapshotRow;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// The listed entries that are symbolic links.
///
/// A link is not a snapshot archive, so a prune never keeps one, never counts one
/// as a store's only snapshot, and never counts the bytes it points at. Selection
/// takes this answer as data rather than asking the filesystem itself, so it
/// stays a pure function of the listing, and the caller reads the filesystem
/// exactly once: here.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LinkedPaths {
    paths: HashSet<PathBuf>,
}

impl LinkedPaths {
    /// Classify `rows`: one `symlink_metadata` per listed entry.
    ///
    /// `symlink_metadata` is `lstat`, so it does not follow the link and a link
    /// to a real archive is recognized as a link instead of as that archive. The
    /// read is metadata only - no archive is opened, no manifest is inflated -
    /// and it is not a second scan of the directory: every path came from the
    /// listing, one entry at a time.
    ///
    /// An entry that cannot be examined at all is left out, so selection treats
    /// it as a real file. That is the safe direction to be wrong in: the listing
    /// already decided the entry is a snapshot, and the delete half re-reads the
    /// entry with `symlink_metadata` before it unlinks anything, so a link is
    /// still never followed even when this read fails.
    #[must_use]
    pub fn among(rows: &[SnapshotRow]) -> Self {
        let paths = rows
            .iter()
            .filter(|row| std::fs::symlink_metadata(&row.path).is_ok_and(|meta| meta.is_symlink()))
            .map(|row| row.path.clone())
            .collect();
        Self { paths }
    }

    /// Whether `path` is one of the linked entries.
    #[must_use]
    pub fn contains(&self, path: &Path) -> bool {
        self.paths.contains(path)
    }
}

#[cfg(test)]
#[path = "prune_kind_tests.rs"]
mod tests;
