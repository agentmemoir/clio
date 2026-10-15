// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the backup manifest type: its field set,
//! deterministic serialization, parsing, and validation.
//!
//! ## Owns
//! - The `clio-store-backup` manifest struct and its JSON (de)serialization
//! - The forward-compatibility rule (unknown fields ignored) and the
//!   `format_version` / `kind` refusal rules
//!
//! ## Does not own
//! - Archive container bytes (see `container`)
//! - Store-specific manifest values (see `clio-store`)
//!
//! ## Boundary
//!
//! Changes to the manifest field set or validation rules should normally
//! require changes here. Changes to how a snapshot is produced should not.
//!
//! Keep this module focused on the manifest type.

use clio_types::{AmError, ErrorCode};

/// The manifest `kind` value this build reads and writes.
pub const KIND: &str = "clio-store-backup";

/// The manifest format version this build supports.
pub const FORMAT_VERSION: u32 = 1;

/// The only compression this build accepts in the archive header.
pub const COMPRESSION: &str = "gzip";

/// Which store was snapshotted. `database` must be a credential-free
/// identifier: the writer records a database name or slug, never a URL that
/// carries a password.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StoreInfo {
    pub backend: String,
    pub slug: String,
    pub database: String,
    pub schema: Option<String>,
}

/// How the snapshot was taken.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SnapshotInfo {
    pub method: String,
    pub consistency: String,
    pub started_at: String,
    pub duration_ms: u64,
}

/// The payload member and what a verify pass must reproduce.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PayloadInfo {
    pub member: String,
    pub bytes: u64,
    pub sha256: String,
    /// The payload member's own size. The member is stored verbatim inside the
    /// archive, so this equals `bytes`; the pair exists so a reader never has to
    /// assume the member was compressed on its own.
    pub uncompressed_bytes: u64,
}

/// Whether the counted set is finished.
///
/// Per-table row counts are not manifest fields. A Postgres snapshot records
/// them in its `dump/meta.json` member, beside the table each count belongs to
/// and the digest of the member that count was measured from; a SQLite
/// snapshot's count is the payload member's own size. The manifest carries
/// only whether the count set is complete, so the counted set can grow without
/// a format-version bump. Unknown fields are ignored on read.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CountsInfo {
    pub complete: bool,
}

/// The key-store member, if the snapshot carries one.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct KeysInfo {
    pub included: bool,
    pub member: Option<String>,
    pub subjects: u64,
    /// The key member's size in bytes, recorded so a restore can reserve room
    /// for it before it stages anything.
    ///
    /// It is a size and not a digest on purpose: the key store's bytes are
    /// copied verbatim and never parsed, so hashing them would mean reading
    /// key material to produce a value nothing needs. Validation requires a
    /// size greater than zero whenever the archive carries the key member; an
    /// archive with no key member records no size, and a reader treats an
    /// absent size as "unknown", not as zero.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
    pub note: String,
}

impl KeysInfo {
    /// Whether the archive carries the key member: the declaration flag and
    /// the member name agree. Validation refuses a manifest where they
    /// disagree, so every later check — validation, extraction, and the
    /// verify pass — reads this one predicate instead of picking a field.
    pub fn carries_key_member(&self) -> bool {
        self.included && self.member.is_some()
    }
}

/// The deployment identity and erasure generation an archive was made under.
///
/// A restore compares this pair with the current erasure authority, so it can
/// refuse an archive from the wrong deployment or one older than the last
/// erasure. The value travels here as plain data: the caller reads the erasure
/// journal and supplies it, and this crate never reads that journal itself.
///
/// The salt fingerprint is a one-way digest of the deployment tombstone salt
/// the archive's member subject ids were hashed under. A restore refuses an
/// archive whose fingerprint is absent or differs from the live authority's,
/// because a different salt makes the recorded subject hashes incomparable.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ManifestAuthority {
    pub deployment_id: String,
    pub generation: u64,
    /// The fingerprint of the salt that hashed the archive's subject ids, or
    /// `None` for an archive written before the field existed. A reader treats
    /// an absent fingerprint as "cannot be verified" and a restore refuses it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub salt_fingerprint: Option<String>,
}

/// The backup manifest: the first archive member, as JSON.
///
/// Field order is the serialization order, so identical manifests
/// serialize to identical bytes. Unknown fields are ignored on read
/// (no `deny_unknown_fields`), so the field set can grow without a
/// format-version bump.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BackupManifest {
    pub kind: String,
    pub format_version: u32,
    pub created_at: String,
    pub tool_version: String,
    pub store: StoreInfo,
    pub snapshot: SnapshotInfo,
    pub payload: PayloadInfo,
    pub counts: CountsInfo,
    pub schema_version: Option<String>,
    pub keys: KeysInfo,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub compression: String,
    /// The erasure authority this archive was made under, when the caller
    /// recorded one. Absent in an archive written before the field existed, so
    /// it is defaulted on read and omitted on write when `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authority: Option<ManifestAuthority>,
}

impl BackupManifest {
    /// Serialize to deterministic JSON bytes.
    pub fn to_json_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("manifest serialization cannot fail")
    }

    /// Parse and validate manifest JSON bytes.
    pub fn from_json_bytes(bytes: &[u8]) -> Result<Self, AmError> {
        let manifest: BackupManifest = serde_json::from_slice(bytes).map_err(|e| {
            AmError::new(
                ErrorCode::ArchiveCorrupt,
                format!("invalid manifest JSON: {e}"),
            )
        })?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Refuse manifests this build cannot trust: wrong kind, unsupported
    /// format version, wrong compression, empty required strings, an
    /// unknown store backend, a malformed payload digest, a key declaration
    /// whose flag and member name disagree, or a key member recorded without
    /// a real size.
    pub fn validate(&self) -> Result<(), AmError> {
        if self.kind != KIND {
            return Err(AmError::new(
                ErrorCode::ArchiveCorrupt,
                format!(
                    "unsupported manifest kind {:?}; this build expects {KIND:?}",
                    self.kind
                ),
            ));
        }
        if self.format_version != FORMAT_VERSION {
            return Err(AmError::new(
                ErrorCode::ArchiveCorrupt,
                format!(
                    "unsupported manifest format_version {}; this build supports {FORMAT_VERSION}",
                    self.format_version
                ),
            ));
        }
        if self.compression != COMPRESSION {
            return Err(AmError::new(
                ErrorCode::ArchiveCorrupt,
                format!(
                    "unsupported manifest compression {:?}; this build expects {COMPRESSION:?}",
                    self.compression
                ),
            ));
        }
        for (value, field) in [
            (&self.created_at, "created_at"),
            (&self.tool_version, "tool_version"),
            (&self.store.backend, "store.backend"),
            (&self.store.slug, "store.slug"),
            (&self.payload.member, "payload.member"),
            (&self.payload.sha256, "payload.sha256"),
        ] {
            if value.is_empty() {
                return Err(AmError::new(
                    ErrorCode::ArchiveCorrupt,
                    format!("manifest field {field} is empty"),
                ));
            }
        }
        if self.store.backend != "sqlite" && self.store.backend != "postgres" {
            return Err(AmError::new(
                ErrorCode::ArchiveCorrupt,
                format!(
                    "unknown store.backend {:?}; this build supports \"sqlite\" and \"postgres\"",
                    self.store.backend
                ),
            ));
        }
        if self.payload.sha256.len() != 64
            || self
                .payload
                .sha256
                .bytes()
                .any(|b| !matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        {
            return Err(AmError::new(
                ErrorCode::ArchiveCorrupt,
                format!(
                    "manifest field payload.sha256 must be 64 lowercase hex characters, found {} bytes",
                    self.payload.sha256.len()
                ),
            ));
        }
        // The declaration flag and the member name must agree: one saying the
        // archive carries the key member while the other says it does not is
        // a manifest no later check could read one way.
        if self.keys.included != self.keys.member.is_some() {
            return Err(AmError::new(
                ErrorCode::ArchiveCorrupt,
                "manifest fields keys.included and keys.member disagree: one declares a key \
                 member and the other declares none",
            ));
        }
        // A key member's recorded size is what a restore reserves room
        // against, so an archive that carries the member must carry a real
        // size; an archive with no key member needs no size at all.
        if self.keys.carries_key_member() {
            match self.keys.bytes {
                None => {
                    return Err(AmError::new(
                        ErrorCode::ArchiveCorrupt,
                        "manifest field keys.bytes is required when the archive carries a key \
                         member",
                    ));
                }
                Some(0) => {
                    return Err(AmError::new(
                        ErrorCode::ArchiveCorrupt,
                        "manifest field keys.bytes must be greater than zero when the archive \
                         carries a key member",
                    ));
                }
                Some(_) => {}
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "manifest_tests.rs"]
mod tests;

/// The manifest authority tests, split out so the field-set tests stay focused.
#[cfg(test)]
#[path = "manifest_authority_tests.rs"]
mod authority_tests;
