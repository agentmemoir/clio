// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for proving one archive is intact: recomputing
//! each member's digest, comparing it with the manifest, and asking the store
//! whether the payload is a real database.
//!
//! ## Owns
//! - `verify_archive` and the `VerifyReport` / `MemberCheck` it returns
//! - The archive facts a report carries forward from the manifest: the id,
//!   the backend, the snapshot method and consistency, and the store schema
//! - The per-member rules: the payload must match its recorded size and
//!   digest, the key member must be present exactly when the manifest says it
//!   is and its streamed size must match the recorded bytes, and a member the
//!   manifest — or a Postgres dump's meta — does not describe is listed, not
//!   trusted
//! - Staging the payload beside the archive so the store can check it
//!
//! ## Does not own
//! - The store's integrity check itself (see `clio-store`; it is injected)
//! - Container bounds and the manifest read (see `container`, `inventory`)
//! - Listing archives (see `inventory`)
//!
//! ## Boundary
//!
//! Changes to what a verify pass checks should normally require changes here.
//! Changes to the manifest fields should not.
//!
//! Keep this module focused on archive verification.

use crate::container::ArchiveReader;
use crate::dump_meta::{DUMP_TABLES_PREFIX, DumpMeta};
use crate::inventory::read_manifest;
use crate::manifest::BackupManifest;
use crate::staging::{self, StagedFile};
use crate::verify_members::{key_check, manifest_check, stage_payload, unlisted_check};
use crate::{KEY_MEMBER, MANIFEST_MEMBER};
use clio_types::{AmError, ErrorCode};
use std::path::{Path, PathBuf};

/// Bytes read from a member's head to recognize a SQLite key-store database.
pub(crate) const MAGIC_LEN: usize = 16;

/// SQLite file magic; the key member must start with it.
pub(crate) const SQLITE_MAGIC: &[u8] = b"SQLite format 3\0";

/// One member's verdict.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct MemberCheck {
    /// The member, or the check performed on it.
    pub member: String,
    /// Whether the member passed.
    pub ok: bool,
    /// What was checked and what was found.
    pub detail: String,
}

/// The result of verifying one archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyReport {
    /// The snapshot id, from the archive's file name.
    pub id: String,
    /// The archive that was verified.
    pub path: PathBuf,
    /// Manifest `created_at`, the ordering time `list` sorts on.
    pub created_at: String,
    /// Manifest `store.backend`.
    pub backend: String,
    /// Manifest `snapshot.method`: how the payload was taken. A report that
    /// omits it cannot say what mechanism a pass is vouching for.
    pub method: String,
    /// Manifest `snapshot.consistency`: the consistency the method gives.
    pub consistency: String,
    /// Manifest `store.schema`, the schema a Postgres dump was scoped to. It is
    /// `None` for a backend that has no schemas of its own.
    pub store_schema: Option<String>,
    /// Manifest `counts.complete`.
    pub counts_complete: bool,
    /// One verdict per member, in archive order.
    pub members: Vec<MemberCheck>,
    /// Members the manifest does not describe, listed rather than trusted.
    pub unlisted: Vec<String>,
    /// True when every check passed.
    pub ok: bool,
}

impl VerifyReport {
    /// The names of the checks that failed, in archive order.
    #[must_use]
    pub fn failed(&self) -> Vec<&str> {
        self.members
            .iter()
            .filter(|check| !check.ok)
            .map(|check| check.member.as_str())
            .collect()
    }
}

/// Verify one archive against its own manifest.
///
/// The payload is re-read once: it is streamed into an owner-only file beside
/// the archive, never a shared temporary directory and never back into the
/// archive, and its digest is taken during that single pass. The store's own
/// integrity check is then called on that file, so a payload that hashes
/// correctly but is not a database is still caught.
///
/// A manifest this build refuses — wrong kind, unknown format version, wrong
/// compression — comes back as an error with its own message rather than a
/// report: there is nothing to compare against. A member that disagrees with
/// the manifest is a report with `ok = false` naming the member.
///
/// # Errors
/// The archive is missing, unreadable, truncated, carries no manifest, or
/// carries a manifest this build refuses.
pub fn verify_archive<F>(path: &Path, integrity: &mut F) -> Result<VerifyReport, AmError>
where
    F: FnMut(&Path) -> Result<(), AmError>,
{
    let manifest = read_manifest(path)?;
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) => {
            return Err(staging::io_error(
                &format!("opening {}", path.display()),
                &error,
            ));
        }
    };
    let mut reader = ArchiveReader::new(file)?;
    // Beside the archive, so the extracted payload lands in the owner-only
    // directory the snapshot already uses.
    let staging_dir = path.parent().unwrap_or(Path::new("."));
    let mut seen = Seen::default();
    let postgres = manifest.store.backend == "postgres";
    // A Postgres archive's table members are declared by its dump meta; their
    // digests are taken as they stream and compared once the meta is out.
    let mut dumped: Vec<(String, u64, String)> = Vec::new();

    while let Some(header) = reader.next_member()? {
        let name = header.name.as_str();
        if name == MANIFEST_MEMBER {
            seen.members.push(manifest_check(&mut reader)?);
        } else if name == manifest.payload.member {
            seen.payload = true;
            let staged = stage_payload(&mut reader, staging_dir, name)?;
            seen.members.push(payload_check(&manifest, &staged));
            if postgres {
                seen.dump_meta = Some(parse_dump_meta(&staged, name)?);
            }
            seen.members.push(integrity_check(name, &staged, integrity));
        } else if name == KEY_MEMBER {
            seen.keys = true;
            seen.members.push(key_check(
                &mut reader,
                manifest.keys.carries_key_member(),
                manifest.keys.bytes,
            )?);
        } else if postgres && name.starts_with(DUMP_TABLES_PREFIX) {
            let digest = reader.read_current(&mut std::io::sink())?;
            dumped.push((name.to_owned(), digest.size, digest.sha256));
        } else {
            seen.members.push(unlisted_check(&mut reader, name)?);
            seen.unlisted.push(name.to_owned());
        }
    }
    let mut members = seen.members;
    members.extend(declared_but_absent(&manifest, seen.payload, seen.keys));
    if let Some(meta) = &seen.dump_meta {
        members.extend(dump_table_checks(meta, &dumped));
        for (check, name) in undeclared_table_checks(meta, &dumped) {
            members.push(check);
            seen.unlisted.push(name);
        }
    }
    let ok = members.iter().all(|check| check.ok);
    Ok(VerifyReport {
        id: crate::archive_name::snapshot_id_of(path).unwrap_or_default(),
        path: path.to_path_buf(),
        created_at: manifest.created_at,
        backend: manifest.store.backend,
        method: manifest.snapshot.method,
        consistency: manifest.snapshot.consistency,
        store_schema: manifest.store.schema,
        counts_complete: manifest.counts.complete,
        members,
        unlisted: seen.unlisted,
        ok,
    })
}

/// What the member walk found, so the closing checks can compare it with what
/// the manifest declared.
#[derive(Default)]
struct Seen {
    members: Vec<MemberCheck>,
    unlisted: Vec<String>,
    payload: bool,
    keys: bool,
    /// The parsed dump meta, when the archive is a Postgres dump and its
    /// member parsed.
    dump_meta: Option<DumpMeta>,
}

/// Parse the staged dump meta member, bounding its size and naming the member
/// on a refusal.
fn parse_dump_meta(staged: &StagedFile, name: &str) -> Result<DumpMeta, AmError> {
    let bytes = crate::dump_meta::read_bounded_meta_bytes(staged.path())
        .map_err(|error| corrupt(name, &error))?;
    DumpMeta::from_json_bytes(&bytes).map_err(|error| corrupt(name, &error))
}

/// One check per dumped table: the member's streamed bytes and digest against
/// the dump meta's record, and every declared member present.
fn dump_table_checks(meta: &DumpMeta, dumped: &[(String, u64, String)]) -> Vec<MemberCheck> {
    let mut out = Vec::with_capacity(meta.tables.len());
    for table in &meta.tables {
        match dumped.iter().find(|(name, _, _)| name == &table.member) {
            Some((_, bytes, sha256)) => {
                let ok = *bytes == table.bytes && sha256 == &table.sha256;
                out.push(MemberCheck {
                    member: table.member.clone(),
                    ok,
                    detail: if ok {
                        format!(
                            "{} bytes, digest {} (both match the dump meta's record for table \
                             {:?})",
                            bytes, sha256, table.table
                        )
                    } else {
                        format!(
                            "the dump meta records {} bytes and digest {} for table {:?}; the \
                             member holds {} bytes and hashes to {}",
                            table.bytes, table.sha256, table.table, bytes, sha256
                        )
                    },
                });
            }
            None => out.push(MemberCheck {
                member: table.member.clone(),
                ok: false,
                detail: format!(
                    "the dump meta declares this member for table {:?}, but the archive has no \
                     such member",
                    table.table
                ),
            }),
        }
    }
    out
}

/// One failing check per dumped table member the dump meta does not declare,
/// each paired with the member name so the caller can list it as unlisted.
///
/// `dump_table_checks` walks the meta, so it reports only the members the meta
/// names. A dumped member the archive carries without a declaration is found
/// the other way round, and it is the mirror of the extraction refusal: an
/// extraction will not take that archive, so a pass that cleared it would vouch
/// for one a restore refuses.
fn undeclared_table_checks(
    meta: &DumpMeta,
    dumped: &[(String, u64, String)],
) -> Vec<(MemberCheck, String)> {
    dumped
        .iter()
        .filter(|(name, _, _)| !meta.tables.iter().any(|table| &table.member == name))
        .map(|(name, bytes, sha256)| {
            (
                MemberCheck {
                    member: name.clone(),
                    ok: false,
                    detail: format!(
                        "{bytes} bytes, digest {sha256}; the dump meta declares no such table \
                         member, so these rows are listed and not trusted"
                    ),
                },
                name.clone(),
            )
        })
        .collect()
}

/// The members the manifest declares but the archive does not carry.
fn declared_but_absent(manifest: &BackupManifest, payload: bool, keys: bool) -> Vec<MemberCheck> {
    let mut out = Vec::new();
    if !payload {
        out.push(MemberCheck {
            member: manifest.payload.member.clone(),
            ok: false,
            detail: "the manifest declares this payload member, but the archive has no such \
                     member"
                .to_owned(),
        });
    }
    if manifest.keys.carries_key_member() && !keys {
        out.push(MemberCheck {
            member: KEY_MEMBER.to_owned(),
            ok: false,
            detail: "the manifest says a key member is included, but the archive has none"
                .to_owned(),
        });
    }
    if !manifest.keys.carries_key_member() && keys {
        out.push(MemberCheck {
            member: KEY_MEMBER.to_owned(),
            ok: false,
            detail: "the archive carries a key member the manifest does not declare".to_owned(),
        });
    }
    out
}

/// The store's own check, called on the extracted payload.
fn integrity_check<F>(name: &str, staged: &StagedFile, integrity: &mut F) -> MemberCheck
where
    F: FnMut(&Path) -> Result<(), AmError>,
{
    let verdict = integrity(staged.path());
    MemberCheck {
        member: format!("{name} integrity"),
        ok: verdict.is_ok(),
        detail: match &verdict {
            Ok(()) => "the store's own integrity check passed".to_owned(),
            Err(error) => format!("the store's own integrity check failed: {error}"),
        },
    }
}

/// Compare a staged payload with the manifest's record of it.
fn payload_check(manifest: &BackupManifest, staged: &StagedFile) -> MemberCheck {
    let digest_matches = staged.sha256() == manifest.payload.sha256;
    let size_matches = staged.bytes() == manifest.payload.bytes;
    let detail = if digest_matches && size_matches {
        format!(
            "{} bytes, digest {} (both match the manifest)",
            staged.bytes(),
            staged.sha256()
        )
    } else {
        let mut parts = Vec::new();
        if !size_matches {
            parts.push(format!(
                "the manifest records {} bytes, the member holds {}",
                manifest.payload.bytes,
                staged.bytes()
            ));
        }
        if !digest_matches {
            parts.push(format!(
                "the manifest records digest {}, the member hashes to {}",
                manifest.payload.sha256,
                staged.sha256()
            ));
        }
        parts.join("; ")
    };
    MemberCheck {
        member: manifest.payload.member.clone(),
        ok: digest_matches && size_matches,
        detail,
    }
}

/// Re-label a container error so it names the member that failed.
pub(crate) fn corrupt(member: &str, error: &AmError) -> AmError {
    AmError::new(
        ErrorCode::ArchiveCorrupt,
        format!("archive member {member:?}: {}", error.message),
    )
}

#[cfg(test)]
#[path = "verify_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "verify_fault_tests.rs"]
mod fault_tests;

/// The verify pass's edge refusals and the payload check's detail.
#[cfg(test)]
#[path = "verify_edges_tests.rs"]
mod edge_tests;

#[cfg(test)]
#[path = "verify_hostile_manifest_tests.rs"]
pub(crate) mod verify_hostile_manifest_tests;

#[cfg(test)]
#[path = "verify_hostile_duplicate_tests.rs"]
mod verify_hostile_duplicate_tests;
