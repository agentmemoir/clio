// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the provenance marker an installed copy of the
//! embedded skill package records beside its assets: what installed it, what it
//! came from, and which bytes it owns.
//!
//! ## Owns
//! - The marker file name and the exact set of facts a marker records
//! - The install state carried beside those facts: the harnesses that reference
//!   the copy and whether the installation created the package directory
//! - The payload digest, taken over the embedded assets and never the marker
//! - The rule that every recorded path is a plain relative path inside the package directory
//! - The canonical byte form, so the same installation always serializes alike
//! - The SHA-256 hex convention the marker and the installation plan share
//!
//! ## Does not own
//! - Deciding whether an installed package is current, edited, or in conflict
//! - Writing the marker or the assets into a skill root
//!
//! ## Boundary
//!
//! Changes to what a marker records, or to the bytes it serializes to, should
//! normally require changes here. Changes to when an installation runs, or where
//! its files live, should not require changes here.
//!
//! The digest deliberately covers the assets only. The marker file is written
//! beside them and is not one of them, so a digest that included the marker
//! would have to contain its own hash. That property is why the payload digest
//! can be compared between an installation and the binary that built it.
//!
//! Keep this module focused on the recorded facts of one installation.

use crate::skill_uninstall_guard::marker_key_is_relative;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

/// The marker file, written inside the installed package directory beside the
/// assets it describes.
pub(crate) const SKILL_MARKER_FILE: &str = ".clio-skill.json";

/// The program recorded as the installer of a package.
pub(crate) const SKILL_INSTALLER: &str = "clio";

/// What one installation recorded about itself.
///
/// The field order below is the serialized order, and the file map is sorted by
/// relative path, so the same installation always produces byte-identical
/// output. A field this version does not know is refused rather than ignored:
/// a marker belongs to whichever program wrote it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SkillMarker {
    /// The program that installed the package.
    pub installer: String,
    /// The version of that program, as it reports itself.
    pub package_version: String,
    /// The catalog hash the schema pack published at install time.
    pub catalog_hash: String,
    /// The digest of the embedded payload, which covers the assets only.
    pub payload_digest: String,
    /// Every file the installation owns: relative path, then the SHA-256 of its
    /// recorded bytes.
    pub files: BTreeMap<String, String>,
    /// Whether this installation created the package directory it wrote into.
    #[serde(default)]
    pub(crate) created_package_dir: bool,
    /// Every harness whose roots reference this package directory.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub(crate) harnesses: BTreeSet<String>,
}

/// Why a marker could not be produced or read.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum MarkerError {
    /// The bytes are not a marker this program wrote.
    Malformed { detail: String },
    /// The marker could not be serialized.
    Unserializable { detail: String },
}

impl SkillMarker {
    /// The marker an installation of `assets` under `catalog_hash` records.
    #[must_use]
    pub(crate) fn from_assets(assets: &[(&str, &str)], catalog_hash: &str) -> Self {
        Self {
            installer: SKILL_INSTALLER.to_owned(),
            package_version: env!("CARGO_PKG_VERSION").to_owned(),
            catalog_hash: catalog_hash.to_owned(),
            payload_digest: payload_digest(assets),
            files: owned_files(assets),
            created_package_dir: true,
            harnesses: BTreeSet::new(),
        }
    }

    /// This marker with the install state recorded beside its assets: whether
    /// this installation created the package directory, and which harnesses
    /// reference it.
    ///
    /// A marker that `recorded` describes keeps the state it already carries, so
    /// a rewrite never forgets the directory's origin or a harness that installed
    /// earlier. With no recorded marker, the state comes from `dir_existed`, which
    /// is false only when this installation made the directory. The `harness` is
    /// added to the set when one is given.
    #[must_use]
    pub(crate) fn with_install_state(
        self,
        recorded: Option<&Self>,
        harness: Option<&str>,
        dir_existed: bool,
    ) -> Self {
        let mut marker = self;
        match recorded {
            Some(existing) => {
                marker.created_package_dir = existing.created_package_dir;
                marker.harnesses.clone_from(&existing.harnesses);
            }
            None => marker.created_package_dir = !dir_existed,
        }
        if let Some(harness) = harness {
            let _ = marker.harnesses.insert(harness.to_owned());
        }
        marker
    }

    /// The canonical bytes of this marker.
    ///
    /// # Errors
    /// The marker cannot be serialized.
    pub(crate) fn to_bytes(&self) -> Result<Vec<u8>, MarkerError> {
        serde_json::to_vec(self).map_err(|error| MarkerError::Unserializable {
            detail: error.to_string(),
        })
    }

    /// The marker `bytes` hold.
    ///
    /// Every recorded path must be a plain relative path inside the package
    /// directory. An absolute path, a parent step, a current step, an empty
    /// string, or the marker's own name is refused, because joining one to the
    /// package directory would leave it or name the marker twice.
    ///
    /// # Errors
    /// The bytes are not a marker, they carry a field this version does not
    /// know, or a recorded path is not a plain relative path.
    pub(crate) fn from_bytes(bytes: &[u8]) -> Result<Self, MarkerError> {
        let marker: Self =
            serde_json::from_slice(bytes).map_err(|error| MarkerError::Malformed {
                detail: error.to_string(),
            })?;
        if let Some(key) = marker.files.keys().find(|key| !marker_key_is_relative(key)) {
            return Err(MarkerError::Malformed {
                detail: format!("recorded path is not a plain relative path: {key}"),
            });
        }
        Ok(marker)
    }
}

/// The digest of the embedded payload: every asset's name and bytes, each
/// length-prefixed and taken in sorted name order, so neither a renamed asset
/// nor a shifted boundary can produce the digest of a different payload.
///
/// The marker file is skipped, so a payload that carries one still digests as
/// the payload without it.
#[must_use]
pub(crate) fn payload_digest(assets: &[(&str, &str)]) -> String {
    let bytes: Vec<(&str, &[u8])> = assets
        .iter()
        .map(|(name, text)| (*name, text.as_bytes()))
        .collect();
    payload_digest_of_bytes(&bytes)
}

/// The digest of a payload given as bytes: every asset's name and bytes, each
/// length-prefixed and taken in sorted name order, so neither a renamed asset nor
/// a shifted boundary can produce the digest of a different payload.
///
/// The marker file is skipped, so a payload that carries one still digests as the
/// payload without it.
#[must_use]
pub(crate) fn payload_digest_of_bytes(assets: &[(&str, &[u8])]) -> String {
    let mut named: Vec<&(&str, &[u8])> = assets
        .iter()
        .filter(|(name, _)| *name != SKILL_MARKER_FILE)
        .collect();
    named.sort_by(|left, right| left.0.cmp(right.0));
    let mut hasher = Sha256::new();
    for (name, bytes) in named {
        hasher.update((name.len() as u64).to_le_bytes());
        hasher.update(name.as_bytes());
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update(bytes);
    }
    hex_of(&hasher.finalize())
}

/// The owned file set of `assets`: every asset's relative path, then the
/// SHA-256 of its bytes. The marker file is skipped, as it is for the digest.
#[must_use]
pub(crate) fn owned_files(assets: &[(&str, &str)]) -> BTreeMap<String, String> {
    assets
        .iter()
        .filter(|(name, _)| *name != SKILL_MARKER_FILE)
        .map(|(name, text)| ((*name).to_owned(), sha256_hex(text.as_bytes())))
        .collect()
}

/// The SHA-256 of `bytes` as lowercase hex, the form a marker records and the
/// form an on-disk file is compared in.
#[must_use]
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    hex_of(&Sha256::digest(bytes))
}

/// The catalog hash this build publishes, read from the schema pack so a marker
/// names the catalog the assets were built against.
#[must_use]
pub(crate) fn catalog_hash() -> String {
    clio_mcp::schema::schema_pack()
        .get("catalog_hash")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .expect("the schema pack publishes a catalog hash")
}

/// Bytes as lowercase hex, two digits per byte, in the one pass the crate's
/// other digest renderer takes.
fn hex_of(bytes: &[u8]) -> String {
    let mut hex = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = std::fmt::Write::write_fmt(&mut hex, format_args!("{byte:02x}"));
    }
    hex
}

#[cfg(test)]
#[path = "skill_marker_tests.rs"]
mod tests;
