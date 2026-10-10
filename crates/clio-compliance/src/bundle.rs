// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the shared JSON export-bundle contract:
//! format identifiers, content-mode enum, entity payload types, the
//! completeness manifest, canonicalization, and content checksums.
//!
//! ## Owns
//! - `BUNDLE_FORMAT` / `BUNDLE_FORMAT_VERSION` constants
//! - `ExportContentMode` (`dsar_plaintext` | `ciphertext_backup`)
//! - `ExportEntities` payload container and `BundleEnvelope` file shape
//! - `CompletenessManifest` fields and construction
//! - Canonical JSON encoding and `content_sha256` computation/verification
//!
//! ## Does not own
//! - Store reads/writes (export.rs serializes, import.rs restores)
//! - Admission gating (clio-admission) or MCP argument decoding (clio-mcp)
//!
//! ## Boundary
//!
//! Changes to the bundle file layout, manifest fields, or canonicalization
//! should normally require changes here. Changes to serialization of one
//! entity kind belong in the export/import modules.
//!
//! Keep this module focused on the portable bundle contract.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::cmp::Ordering;

use clio_store::{AssocEdgeRecord, ComplianceTombstoneView};
use clio_types::{
    AmError, BeliefObject, ErrorCode, FailureRecord, MemoryItem, PersonaPreference,
    PersonaStableEntry, TaskRecord, TripleRecord,
};

/// Bundle format discriminator (`format` field).
pub const BUNDLE_FORMAT: &str = "clio.memory_export";

/// First-release bundle format version; import rejects unsupported versions.
pub const BUNDLE_FORMAT_VERSION: u64 = 1;

/// `export_content_mode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExportContentMode {
    /// Authorized decrypt → readable plaintext content (default).
    #[serde(rename = "dsar_plaintext")]
    DsarPlaintext,
    /// Opaque content bodies + metadata; no DEK material in the file.
    #[serde(rename = "ciphertext_backup")]
    CiphertextBackup,
}

impl ExportContentMode {
    /// Stable wire string.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DsarPlaintext => "dsar_plaintext",
            Self::CiphertextBackup => "ciphertext_backup",
        }
    }

    /// Parse from caller input (defaults empty to the DSAR default).
    ///
    /// # Errors
    /// Unknown mode label.
    pub fn parse(raw: Option<&str>) -> Result<Self, AmError> {
        match raw.unwrap_or("") {
            "" | "dsar_plaintext" => Ok(Self::DsarPlaintext),
            "ciphertext_backup" => Ok(Self::CiphertextBackup),
            other => Err(AmError::new(
                ErrorCode::InvalidArgument,
                format!("content_mode must be dsar_plaintext|ciphertext_backup, got `{other}`"),
            )),
        }
    }
}

/// One opaque content body for `ciphertext_backup` mode.
///
/// Metadata inventory stays complete; the content body is the
/// exporting node's at-rest ciphertext with no DEK material in the file.
/// Decrypted-content restore requires shared key custody and is out of the
/// first release.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OpaqueItem {
    /// Stable item id.
    pub id: String,
    /// Data-subject id for key resolution.
    pub subject_id: String,
    /// `semantic` | `episodic`.
    pub kind: String,
    /// Semantic category, when semantic.
    pub category: Option<String>,
    /// Episodic type tag, when episodic.
    pub episodic_type: Option<String>,
    /// `fact` | `belief`.
    pub epistemic_kind: String,
    /// Provenance source type (beliefs).
    pub source_type: Option<String>,
    /// Confidence.
    pub confidence: Option<f64>,
    /// Five-factor admission score.
    pub admission_score: Option<f64>,
    /// Evidence reference.
    pub source_ref: Option<String>,
    /// Valid-time start.
    pub valid_from: String,
    /// Valid-time end.
    pub valid_until: Option<String>,
    /// Transaction-time start.
    pub tx_from: String,
    /// Transaction-time end.
    pub tx_until: Option<String>,
    /// Creation timestamp.
    pub created_at: String,
    /// Last-update timestamp.
    pub updated_at: String,
    /// Opaque at-rest content bytes from the exporting node (no DEK material).
    pub content_cipher: String,
}

/// One portable memory entity set (payload list).
///
/// All members are content-free-of-secrets after export masking; the file
/// carries no DEKs, API keys, or sync credentials.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ExportEntities {
    /// Memory items with content halves (snapshot/gist refs + bodies).
    #[serde(default)]
    pub items: Vec<MemoryItem>,
    /// Opaque content items (`ciphertext_backup` mode only).
    #[serde(default)]
    pub opaque_items: Vec<OpaqueItem>,
    /// Bi-temporal SPO triples (open and closed/superseded).
    #[serde(default)]
    pub triples: Vec<TripleRecord>,
    /// Graph/structure association edges (`assoc_edge`, both kinds).
    #[serde(default)]
    pub assoc_edges: Vec<AssocEdgeRecord>,
    /// Persona stable entries (bank injected from import args; bank skipped here).
    #[serde(default)]
    pub persona_stable: Vec<PersonaStableEntry>,
    /// Persona continuous preference states.
    #[serde(default)]
    pub persona_preferences: Vec<PersonaPreference>,
    /// Task history records.
    #[serde(default)]
    pub tasks: Vec<TaskRecord>,
    /// Failure history records.
    #[serde(default)]
    pub failures: Vec<FailureRecord>,
    /// Belief objects with full confidence trajectories.
    #[serde(default)]
    pub beliefs: Vec<BeliefObject>,
    /// Compliance tombstone metadata for erased subjects (content-free).
    #[serde(default)]
    pub tombstones: Vec<ComplianceTombstoneView>,
}

impl ExportEntities {
    /// Entity count by manifest type label.
    #[must_use]
    pub fn counts_by_type(&self) -> Vec<(String, usize)> {
        vec![
            ("items".into(), self.items.len() + self.opaque_items.len()),
            ("triples".into(), self.triples.len()),
            ("assoc_edges".into(), self.assoc_edges.len()),
            ("persona_stable".into(), self.persona_stable.len()),
            ("persona_preferences".into(), self.persona_preferences.len()),
            ("tasks".into(), self.tasks.len()),
            ("failures".into(), self.failures.len()),
            ("beliefs".into(), self.beliefs.len()),
            ("tombstones".into(), self.tombstones.len()),
        ]
    }
}

/// Versioned inventory sibling of every export.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompletenessManifest {
    /// Bundle format version (matches the envelope).
    pub format_version: u64,
    /// Content mode of the bundle.
    pub content_mode: ExportContentMode,
    /// Bank ids covered by the export.
    pub bank_ids: Vec<String>,
    /// Row counts by entity type.
    pub counts_by_type: Vec<(String, usize)>,
    /// Row counts by semantic category / episodic tag (as applicable).
    pub counts_by_category: Vec<(String, usize)>,
    /// SHA-256 hex over the canonical entity bytes.
    pub content_sha256: String,
    /// Filter describing the selection; empty object means full.
    pub filter: Value,
    /// RFC 3339 UTC generation timestamp.
    pub generated_at: String,
    /// False when filtered/truncated; `omissions` must then explain why.
    pub complete: bool,
    /// Omission notes (required when `complete` is false).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub omissions: Vec<String>,
}

/// Single-file bundle layout.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BundleEnvelope {
    /// Always [`BUNDLE_FORMAT`].
    pub format: String,
    /// Bundle format version.
    pub format_version: u64,
    /// Content mode of the payload.
    pub content_mode: ExportContentMode,
    /// Completeness manifest (same file object).
    pub manifest: CompletenessManifest,
    /// Portable entity payload.
    pub entities: ExportEntities,
}

/// SHA-256 hex over arbitrary bytes.
#[must_use]
pub fn sha256_hex_bytes(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Canonical entity bytes: UTF-8, object keys sorted
/// lexicographically at every level, no insignificant whitespace, entity
/// arrays in documented stable order (sorted here by stable id, then
/// `key`/`as_of` for the id-less persona rows, then tombstone identity).
///
/// Canonicalization passes through `serde_json::Value` (its `Map` is a
/// `BTreeMap`, so key ordering is lexicographic at every level) and compact
/// serialization drops whitespace. Typed struct serialization alone emits
/// declaration order, which is NOT canonical — hence the round-trip.
/// Infallible for `ExportEntities`: every float is validated finite at the
/// store boundary and object keys are strings.
#[must_use]
pub fn canonical_entities_bytes(entities: &ExportEntities) -> Vec<u8> {
    let mut value = serde_json::to_value(entities)
        .expect("ExportEntities serialization cannot fail: floats are store-validated");
    sort_entity_arrays(&mut value);
    serde_json::to_vec(&value).expect("canonical Value serialization cannot fail")
}

/// Canonical bytes of an already-parsed `entities` value (same rules).
///
/// Applied to the raw file payload on import so a bundle whose arrays were
/// written in non-canonical order still verifies against the manifest.
#[must_use]
pub fn canonical_value_bytes(value: &Value) -> Vec<u8> {
    let mut value = value.clone();
    sort_entity_arrays(&mut value);
    serde_json::to_vec(&value).expect("canonical Value serialization cannot fail")
}

/// Order every entity array by its stable identity fields so exporter and
/// importer canonicalize to identical bytes regardless of input order.
///
/// Primary keys run `id`, then `key`/`as_of` (id-less persona rows), then the
/// tombstone identity `item_id_hash`/`request_id`/`subject_hash`. A `null` or
/// absent optional field sorts last, matching the SQL tombstone read order. A
/// final full-value comparison makes the order total, so rows that tie on every
/// identity field cannot leak backend read order into the checksum.
fn sort_entity_arrays(value: &mut Value) {
    let Value::Object(map) = value else {
        return;
    };
    for v in map.values_mut() {
        let Value::Array(arr) = v else {
            continue;
        };
        arr.sort_by(compare_entity);
    }
}

/// Total order over two entity values (see [`sort_entity_arrays`]).
fn compare_entity(a: &Value, b: &Value) -> Ordering {
    for field in [
        "id",
        "key",
        "as_of",
        "item_id_hash",
        "request_id",
        "subject_hash",
    ] {
        match compare_opt_str(a, b, field) {
            Ordering::Equal => {}
            other => return other,
        }
    }
    a.to_string().cmp(&b.to_string())
}

/// Compare an optional string field; `null` or absent sorts last.
fn compare_opt_str(a: &Value, b: &Value, field: &str) -> Ordering {
    match (
        a.get(field).and_then(Value::as_str),
        b.get(field).and_then(Value::as_str),
    ) {
        (Some(a), Some(b)) => a.cmp(b),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

/// SHA-256 hex over canonical bytes of an entity payload.
#[must_use]
pub fn entities_checksum(entities: &ExportEntities) -> String {
    sha256_hex_bytes(&canonical_entities_bytes(entities))
}

/// Validate a parsed envelope's format identity before any other work.
///
/// # Errors
/// Missing/mismatched `format`, missing/unsupported `format_version`, or
/// missing manifest.
pub fn validate_envelope(envelope: &BundleEnvelope) -> Result<(), AmError> {
    if envelope.format != BUNDLE_FORMAT {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            format!(
                "unsupported bundle format `{}`; expected `{BUNDLE_FORMAT}`",
                envelope.format
            ),
        ));
    }
    if envelope.format_version == 0 || envelope.format_version > BUNDLE_FORMAT_VERSION {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            format!(
                "unsupported bundle format_version {}; this build supports 1..={BUNDLE_FORMAT_VERSION}",
                envelope.format_version
            ),
        ));
    }
    Ok(())
}

/// Parsed source file for import (kept as raw JSON for checksum verification).
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedBundle {
    /// The validated envelope (format identity already checked).
    pub envelope: BundleEnvelope,
    /// Raw `entities` value as parsed from the file, for checksum recompute.
    pub raw_entities: Value,
}

/// Parse and structurally validate a bundle file before any write.
///
/// Rejects: non-object JSON, wrong `format`, unsupported `format_version`.
/// Checksum comparison happens separately.
///
/// # Errors
/// Parse or format-identity failures.
pub fn parse_bundle(raw: &Value) -> Result<ParsedBundle, AmError> {
    let envelope: BundleEnvelope = serde_json::from_value(raw.clone()).map_err(|e| {
        AmError::new(
            ErrorCode::InvalidArgument,
            format!("bundle envelope invalid: {e}"),
        )
    })?;
    validate_envelope(&envelope)?;
    // The envelope parsed from `raw`, so `entities` must be present in it.
    let raw_entities = raw
        .get("entities")
        .cloned()
        .expect("parsed envelope guarantees an entities object");
    Ok(ParsedBundle {
        envelope,
        raw_entities,
    })
}

#[cfg(test)]
#[path = "bundle_tests.rs"]
mod bundle_tests;
