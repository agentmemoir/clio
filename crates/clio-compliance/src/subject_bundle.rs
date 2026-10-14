// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the internal subject bundle: building one
//! subject's selection from every planned family row, serializing it as
//! versioned canonical bytes, and pairing those bytes with an in-memory
//! manifest that carries the checksum, the exact family counts, and every
//! named omission.
//!
//! ## Owns
//! - `build_subject_bundle` and the `SubjectBundle` bytes-plus-manifest result
//! - `SubjectBundleManifest` and its checksum verification
//! - The subject selection byte format and its version constant
//!
//! ## Does not own
//! - The family selection itself (`inventory_map` supplies it)
//! - The checksum and canonicalization primitives (`bundle`)
//! - Any public export surface: no CLI, MCP, or file destination exists here,
//!   and the public export filter gains no subject selector
//!
//! ## Boundary
//!
//! Changes to what the bundle bytes contain or how the manifest verifies
//! should normally require changes here. Changes to how a family selects
//! records belong in `inventory_map`; changes to the public export path
//! belong in `export`.
//!
//! Keep this module focused on the internal subject bundle.

// The bundle is internal and nothing in production calls it yet: this module
// and its tests are its only consumers. A later gate owns the first caller.
#![allow(dead_code)]

pub(crate) use super::inventory_map::{FamilyClass, FamilySelection, SubjectFamilyInput};
use super::inventory_map::{SubjectFamilyMapping, subject_family_mapping};
use crate::bundle::sha256_hex_bytes;
use crate::inventory::InventoryBackend;
use clio_types::{AmError, ErrorCode};
use serde_json::Value;

/// Byte-format discriminator of the subject selection bytes.
const SUBJECT_BUNDLE_FORMAT: &str = "clio.subject_selection";

/// First version of the subject selection byte format.
const SUBJECT_BUNDLE_FORMAT_VERSION: u64 = 1;

/// One subject's bundle: versioned selection bytes plus their manifest.
///
/// In memory only. No destination path exists on this type and no function
/// here writes a file, so no production destination can be produced.
pub(crate) struct SubjectBundle {
    /// The versioned selection bytes, canonical JSON.
    pub(crate) bytes: Vec<u8>,
    /// The manifest describing [`SubjectBundle::bytes`].
    pub(crate) manifest: SubjectBundleManifest,
}

/// The manifest of one subject bundle.
///
/// Every field describes the same one mapping run the bytes were built from,
/// so the counts, the omissions, and the checksum always describe the same
/// bytes.
pub(crate) struct SubjectBundleManifest {
    /// Byte-format version of the selection bytes.
    pub(crate) format_version: u64,
    /// Which store backend the selection describes.
    pub(crate) backend: InventoryBackend,
    /// The bank the subject's records were read from.
    pub(crate) bank_id: String,
    /// The data subject the bundle selects for.
    pub(crate) subject_id: String,
    /// SHA-256 hex over the selection bytes; changed bytes fail.
    pub(crate) content_sha256: String,
    /// Byte size of the selection bytes.
    pub(crate) content_bytes: u64,
    /// One selection per planned family row, in report order.
    pub(crate) families: Vec<FamilySelection>,
    /// The records all families selected for the subject.
    pub(crate) selected_total: u64,
    /// The store-wide measured rows no family selected for the subject.
    pub(crate) omitted_total: u64,
    /// True iff every family is complete. Copied from the mapping: an
    /// internal store-wide accounting flag, always false under the fixed
    /// plan, and never the export completeness verdict, which the export
    /// path computes from its own conditions.
    pub(crate) complete: bool,
    /// Every named omission, prefixed with its family.
    pub(crate) omissions: Vec<String>,
}

impl SubjectBundle {
    /// Recompute the checksum over the carried bytes and compare it with the
    /// manifest's. Any changed byte fails.
    ///
    /// # Errors
    /// A checksum mismatch between the manifest and the bytes.
    pub(crate) fn verify_checksum(&self) -> Result<(), AmError> {
        let recomputed = sha256_hex_bytes(&self.bytes);
        if recomputed == self.manifest.content_sha256 {
            return Ok(());
        }
        Err(AmError::new(
            ErrorCode::InvalidArgument,
            format!(
                "subject bundle checksum mismatch: manifest {}, bytes {recomputed}",
                self.manifest.content_sha256
            ),
        ))
    }
}

/// Build one subject's bundle from every planned family row on one store.
///
/// The family selection is read once; the bytes, the counts, the omissions,
/// and the checksum all describe that one selection, so a store change during
/// construction cannot mix two states into one bundle.
///
/// # Errors
/// Storage read failures from the store's own public reads.
pub(crate) fn build_subject_bundle(
    input: &SubjectFamilyInput<'_>,
) -> Result<SubjectBundle, AmError> {
    let mapping = subject_family_mapping(input)?;
    let bytes = selection_bytes(input.bank_id, &mapping);
    let manifest = SubjectBundleManifest {
        format_version: SUBJECT_BUNDLE_FORMAT_VERSION,
        backend: mapping.backend,
        bank_id: input.bank_id.to_owned(),
        subject_id: mapping.subject_id.clone(),
        content_sha256: sha256_hex_bytes(&bytes),
        content_bytes: bytes.len() as u64,
        selected_total: mapping.selected_total,
        omitted_total: mapping.omitted_total,
        complete: mapping.complete,
        omissions: mapping.omissions.clone(),
        families: mapping.families,
    };
    Ok(SubjectBundle { bytes, manifest })
}

/// The versioned selection bytes: one canonical JSON object naming the
/// format, the version, the mapped subject and bank, the backend, and one
/// selection per planned family. Keys are sorted because the JSON value map
/// is a `BTreeMap`, and no value is a float, so the bytes are deterministic.
fn selection_bytes(bank_id: &str, mapping: &SubjectFamilyMapping) -> Vec<u8> {
    let value = serde_json::json!({
        "format": SUBJECT_BUNDLE_FORMAT,
        "format_version": SUBJECT_BUNDLE_FORMAT_VERSION,
        "bank_id": bank_id,
        "subject_id": mapping.subject_id,
        "backend": mapping.backend,
        "families": mapping.families.iter().map(family_json).collect::<Vec<Value>>(),
    });
    serde_json::to_vec(&value).expect("subject selection serialization cannot fail: no floats")
}

/// One family's selection as a JSON object with a stable field set.
fn family_json(selection: &FamilySelection) -> Value {
    serde_json::json!({
        "family": selection.family,
        "class": class_label(selection.class),
        "selected": selection.selected,
        "selected_ids": selection.selected_ids,
        "omitted": selection.omitted,
        "omission": selection.omission,
        "complete": selection.complete,
    })
}

/// The stable wire label of one family class.
fn class_label(class: FamilyClass) -> &'static str {
    match class {
        FamilyClass::Selected => "selected",
        FamilyClass::OtherOnly => "other_only",
        FamilyClass::InseparableShared => "inseparable_shared",
        FamilyClass::UnknownLink => "unknown_link",
        FamilyClass::NotStored => "not_stored",
    }
}

#[cfg(test)]
#[path = "subject_bundle_tests.rs"]
mod subject_bundle_tests;

#[cfg(test)]
#[path = "subject_bundle_pg_tests.rs"]
mod subject_bundle_pg_tests;
