// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for assembling one snapshot archive: building
//! its manifest, streaming its members in the fixed order, and publishing it
//! atomically.
//!
//! ## Owns
//! - `CreateRequest` / `CreateReport` and the two-pass write they describe
//! - The member order (manifest, payload, additional members, key member) and
//!   the manifest values
//! - Bounding and control-character stripping for `--label` and `--note`
//!
//! ## Does not own
//! - Producing the payload (see `clio-store`'s snapshot half)
//! - Container bytes and member order enforcement (see `container`)
//! - Member staging (see `staging`) and the atomic publish (see `atomic_publish`)
//! - Listing and verifying archives (see `inventory`, `verify`)
//!
//! ## Boundary
//!
//! Changes to what a created archive contains should normally require changes
//! here. Changes to the container encoding should not.
//!
//! Keep this module focused on the create pipeline.

use crate::KEY_MEMBER;
use crate::archive_name::{self, collision_suffix};
use crate::atomic_publish::AtomicFile;
use crate::container::ArchiveWriter;
use crate::manifest::{
    BackupManifest, COMPRESSION, CountsInfo, FORMAT_VERSION, KIND, KeysInfo, PayloadInfo,
    SnapshotInfo, StoreInfo,
};
use crate::staging::{self, StagedFile};
use clio_types::{AmError, ErrorCode};
use std::path::PathBuf;

/// Longest `--label` or `--note` kept in the manifest, in characters.
pub const MAX_OPERATOR_TEXT: usize = 200;

/// How many names one create tries before giving up on a free file name.
const NAME_ATTEMPTS: u32 = 8;

/// The key store a snapshot should carry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeySource {
    /// File to copy verbatim as the key member.
    pub path: PathBuf,
    /// How many subject slots it holds.
    pub subjects: u64,
}

/// One named staged member written between the payload and the key member.
///
/// A Postgres dump's table members travel this way: the manifest's payload
/// member is `dump/meta.json`, and each table's `COPY` output is one
/// additional member in the dump's numbering order.
#[derive(Debug)]
pub struct StagedMember {
    /// The member's name inside the archive.
    pub name: String,
    /// The staged, measured file.
    pub file: StagedFile,
}

/// Everything one create needs, with the payload already staged and measured.
///
/// The payload arrives staged because only the store half can produce it, and
/// it arrives measured because the manifest has to be complete before it is
/// written, while the payload's size and digest only exist once the payload
/// exists. Both facts are why the write is two-pass.
#[derive(Debug)]
pub struct CreateRequest {
    /// Directory the archive is written into.
    pub backup_dir: PathBuf,
    /// Credential-free store slug; the id starts with it.
    pub slug: String,
    /// Backend the snapshot came from (`sqlite`).
    pub backend: String,
    /// Credential-free database identifier recorded in the manifest.
    pub database: String,
    /// Schema name, when the backend has one.
    pub store_schema: Option<String>,
    /// The store's recorded schema version.
    pub schema_version: Option<String>,
    /// The staged, measured payload member.
    pub payload: StagedFile,
    /// Member name for the payload.
    pub payload_member: String,
    /// Members written after the payload, in this order, before the key
    /// member. A Postgres dump carries its table members here; a SQLite
    /// payload carries none.
    pub additional_members: Vec<StagedMember>,
    /// How the snapshot was taken.
    pub method: String,
    /// The consistency guarantee the mechanism gives.
    pub consistency: String,
    /// UTC stamp taken before the copy started.
    pub started_at: String,
    /// Copy duration in milliseconds.
    pub duration_ms: u64,
    /// True when the counted row set was exhaustive.
    pub counts_complete: bool,
    /// The key store, or `None` when the target keeps keys in memory.
    pub keys: Option<KeySource>,
    /// Operator label, verbatim after cleaning.
    pub label: Option<String>,
    /// Operator note, verbatim after cleaning.
    pub note: Option<String>,
    /// The tool that wrote the archive.
    pub tool_version: String,
}

/// What a finished create reports.
#[derive(Debug, Clone, PartialEq)]
pub struct CreateReport {
    /// The snapshot id: the archive's file-name stem.
    pub id: String,
    /// Absolute path of the published archive.
    pub path: PathBuf,
    /// Bytes of the published archive.
    pub archive_bytes: u64,
    /// Bytes of the payload member.
    pub payload_bytes: u64,
    /// Member bytes in total before compression: the manifest, the payload, and
    /// the key member when the archive carries one. Tar headers and padding are
    /// not counted, so this is the content the gzip stream compresses.
    pub uncompressed_bytes: u64,
    /// `uncompressed_bytes / archive_bytes`; zero when the archive is empty.
    pub compression_ratio: f64,
    /// Copy duration in milliseconds.
    pub duration_ms: u64,
    /// Whether the archive carries a key member.
    pub keys_included: bool,
    /// How many subject slots the carried key store holds.
    pub keys_subjects: u64,
}

/// Assemble one archive and publish it atomically.
///
/// The write is two passes, and the split is forced by the format: the manifest
/// is the first member *and* must be complete, but it records the payload's size
/// and digest, which do not exist until the payload has been written. So the
/// payload and the key store are staged and measured first, the manifest is
/// serialized from those facts, and only then is the archive written —
/// manifest first, then each staged file copied in as a member. The assembly
/// pass copies the digests pass one recorded; it never re-measures a member.
///
/// Everything staged lives in the backup directory, so every later rename stays
/// on one filesystem and a staged key store never sits outside the owner-only
/// archive. The archive itself is written to a temporary file in that same
/// directory, flushed, and renamed, so a listing never shows a partial archive
/// and a crash leaves a temporary file rather than a broken snapshot.
///
/// # Errors
/// The slug cannot name a file, the manifest is rejected by its own validation,
/// a member cannot be staged or written, or the archive cannot be published.
pub fn create_archive(request: &CreateRequest) -> Result<CreateReport, AmError> {
    archive_name::check_slug(&request.slug)?;
    // Pass one for the key member: staged in the backup directory, copied
    // verbatim, digested once, and removed on every path out of here.
    let keys = match &request.keys {
        Some(source) => Some((
            stage_key_member(source, &request.backup_dir)?,
            source.subjects,
        )),
        None => None,
    };
    let manifest = build_manifest(request, keys.as_ref())?;
    let manifest_bytes = manifest.to_json_bytes();
    let (id, final_path) = reserve_name(request)?;

    let mut atomic = AtomicFile::reserve(final_path, "archive")?;
    let additional_bytes: u64 = request
        .additional_members
        .iter()
        .map(|member| member.file.bytes())
        .sum();
    let uncompressed_bytes = manifest_bytes.len() as u64
        + request.payload.bytes()
        + additional_bytes
        + keys.as_ref().map_or(0, |(staged, _)| staged.bytes());
    let mut writer = ArchiveWriter::new(atomic.take_writer()?);
    // The container's own failures already name the member they happened on; a
    // staged file that cannot be reopened is named here, so a failure always
    // says which member it was.
    writer.write_manifest(&manifest_bytes)?;
    let payload = match request.payload.open() {
        Ok(file) => file,
        Err(error) => return Err(member_error(&request.payload_member, &error)),
    };
    writer.add_measured_member(&request.payload_member, payload, request.payload.bytes())?;
    for member in &request.additional_members {
        let file = match member.file.open() {
            Ok(file) => file,
            Err(error) => return Err(member_error(&member.name, &error)),
        };
        writer.add_measured_member(&member.name, file, member.file.bytes())?;
    }
    if let Some((staged, _)) = keys.as_ref() {
        let key_file = match staged.open() {
            Ok(file) => file,
            Err(error) => return Err(member_error(KEY_MEMBER, &error)),
        };
        writer.add_measured_member(KEY_MEMBER, key_file, staged.bytes())?;
    }
    let archive = writer.finish()?;
    let archive_bytes = match archive.metadata() {
        Ok(meta) => meta.len(),
        Err(error) => return Err(staging::io_error("measuring the finished archive", &error)),
    };
    let path = atomic.commit(archive)?;
    Ok(CreateReport {
        id,
        path,
        archive_bytes,
        payload_bytes: request.payload.bytes(),
        uncompressed_bytes,
        compression_ratio: ratio(uncompressed_bytes, archive_bytes),
        duration_ms: request.duration_ms,
        keys_included: keys.is_some(),
        keys_subjects: keys.as_ref().map_or(0, |(_, subjects)| *subjects),
    })
}

/// The compression ratio, or zero when the archive is empty.
fn ratio(uncompressed: u64, archive: u64) -> f64 {
    if archive == 0 {
        return 0.0;
    }
    #[allow(clippy::cast_precision_loss)]
    let value = uncompressed as f64 / archive as f64;
    (value * 100.0).round() / 100.0
}

/// Stage the key store in the backup directory: copied as it stands, digested
/// once, and removed on every path out of the create.
///
/// What is copied is a key store whose per-subject keys are already wrapped
/// under the Clio encryption key, and the Clio key is not in it, so the staged
/// bytes are ciphertext rather than keys. The copy is still byte-for-byte: this
/// is how a key store travels with a snapshot, and nothing here parses or
/// rewrites what it copies.
///
/// Staging inside the backup directory is not a convenience: it keeps key
/// material on the same owner-only filesystem as the archive it belongs in, so
/// a staged key store never sits in a shared temporary directory.
fn stage_key_member(key: &KeySource, backup_dir: &std::path::Path) -> Result<StagedFile, AmError> {
    match staging::stage_copy(&key.path, backup_dir, "keys") {
        Ok(staged) => Ok(staged),
        Err(error) => Err(member_error(KEY_MEMBER, &error)),
    }
}

/// Build the manifest from the measured facts.
fn build_manifest(
    request: &CreateRequest,
    keys: Option<&(StagedFile, u64)>,
) -> Result<BackupManifest, AmError> {
    let keys_info = match keys {
        Some((staged, subjects)) => KeysInfo {
            included: true,
            member: Some(KEY_MEMBER.to_owned()),
            subjects: *subjects,
            // The size the copy already measured, read from the staging record
            // rather than by measuring the file again.
            bytes: Some(staged.bytes()),
            note: "key store copied; per-subject keys are wrapped and the Clio key is not included"
                .to_owned(),
        },
        None => KeysInfo {
            included: false,
            member: None,
            subjects: 0,
            bytes: None,
            note: "this store keeps its keys in process memory, so no key store file \
                   exists to carry; a restore needs the key store from elsewhere"
                .to_owned(),
        },
    };
    let manifest = BackupManifest {
        kind: KIND.to_owned(),
        format_version: FORMAT_VERSION,
        created_at: archive_name::rfc3339_utc(archive_name::now_seconds()),
        tool_version: request.tool_version.clone(),
        store: StoreInfo {
            backend: request.backend.clone(),
            slug: request.slug.clone(),
            database: request.database.clone(),
            schema: request.store_schema.clone(),
        },
        snapshot: SnapshotInfo {
            method: request.method.clone(),
            consistency: request.consistency.clone(),
            started_at: request.started_at.clone(),
            duration_ms: request.duration_ms,
        },
        payload: PayloadInfo {
            member: request.payload_member.clone(),
            bytes: request.payload.bytes(),
            sha256: request.payload.sha256().to_owned(),
            // The payload member is stored verbatim inside the archive, so its
            // uncompressed size is its size.
            uncompressed_bytes: request.payload.bytes(),
        },
        counts: CountsInfo {
            complete: request.counts_complete,
        },
        schema_version: request.schema_version.clone(),
        keys: keys_info,
        label: clean_text(request.label.as_deref()),
        note: clean_text(request.note.as_deref()),
        compression: COMPRESSION.to_owned(),
    };
    manifest.validate()?;
    Ok(manifest)
}

/// Pick a free archive path: the id, then the file name around it.
fn reserve_name(request: &CreateRequest) -> Result<(String, PathBuf), AmError> {
    let seconds = archive_name::now_seconds();
    for _ in 0..NAME_ATTEMPTS {
        let id = archive_name::snapshot_id(&request.slug, seconds, &collision_suffix());
        let name = archive_name::archive_file_name(&id);
        // A name that is not a plain file name never reaches the filesystem.
        if name.contains('/') || name.contains('\\') {
            continue;
        }
        let path = request.backup_dir.join(&name);
        if !path.exists() {
            return Ok((id, path));
        }
    }
    Err(AmError::new(
        ErrorCode::Internal,
        format!(
            "could not find a free snapshot name in {}",
            request.backup_dir.display()
        ),
    ))
}

/// Strip control characters and bound operator text.
///
/// `--label` and `--note` are stored in a file that is designed to be copied off
/// this machine, so a newline cannot smuggle a second record into a viewer and
/// the length is bounded.
fn clean_text(raw: Option<&str>) -> Option<String> {
    let text = raw?;
    let cleaned: String = text
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_OPERATOR_TEXT)
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

/// Add the member name to a container error, so a failure names the member.
fn member_error(member: &str, error: &AmError) -> AmError {
    AmError::new(
        error.code,
        format!("archive member {member:?}: {}", error.message),
    )
}

#[cfg(test)]
#[path = "create_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "create_refusal_tests.rs"]
mod refusal_tests;

/// The key size the create records in the manifest, which a restore's space
/// pre-flight reserves against.
#[cfg(test)]
#[path = "create_key_size_tests.rs"]
mod key_size_tests;

#[cfg(test)]
#[path = "create_postgres_archive_tests.rs"]
mod create_postgres_archive_tests;
