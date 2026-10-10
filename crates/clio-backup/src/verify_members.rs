// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the verify pass's per-member streaming
//! checks: reading a member's bytes off the open archive and turning what was
//! read into one verdict.
//!
//! ## Owns
//! - `manifest_check`, `key_check`, and `unlisted_check`: the three members a
//!   verify pass reads or skips without staging
//! - `stage_payload`: streaming the open payload member into an owner-only
//!   staged file and adopting it only on a clean read
//!
//! ## Does not own
//! - The pass itself and the report it assembles (see `verify`)
//! - Comparing a staged member with a declared record (see `verify`)
//! - Container bounds and the staging lifecycle (see `container_reader` and
//!   `staging`)
//!
//! ## Boundary
//!
//! Changes to how one member is read while verifying should normally require
//! changes here. Changes to what the pass compares should not require changes
//! here.
//!
//! Keep this module focused on the verify pass's per-member reads.

use crate::container::ArchiveReader;
use crate::extract::staging_failure;
use crate::head_sink::HeadSink;
use crate::staging::{self, StagedFile};
use crate::verify::{MAGIC_LEN, MemberCheck, SQLITE_MAGIC, corrupt};
use crate::{KEY_MEMBER, MANIFEST_MEMBER};
use clio_types::AmError;
use std::path::Path;

/// The manifest member: already parsed and validated, so only its size and
/// digest are reported here.
pub(crate) fn manifest_check(
    reader: &mut ArchiveReader<std::fs::File>,
) -> Result<MemberCheck, AmError> {
    let digest = match reader.read_current(&mut std::io::sink()) {
        Ok(digest) => digest,
        Err(error) => return Err(corrupt(MANIFEST_MEMBER, &error)),
    };
    Ok(MemberCheck {
        member: MANIFEST_MEMBER.to_owned(),
        ok: true,
        detail: format!(
            "{} bytes, digest {} (parsed and validated before this pass)",
            digest.size, digest.sha256
        ),
    })
}

/// The key member: its head must be a SQLite key-store database, its presence
/// must match what the manifest declared, and its streamed size must match the
/// size the manifest recorded.
pub(crate) fn key_check(
    reader: &mut ArchiveReader<std::fs::File>,
    carries: bool,
    recorded: Option<u64>,
) -> Result<MemberCheck, AmError> {
    let mut head = HeadSink::new(MAGIC_LEN);
    let digest = match reader.read_current(&mut head) {
        Ok(digest) => digest,
        Err(error) => return Err(corrupt(KEY_MEMBER, &error)),
    };
    let is_key_store = head.head() == SQLITE_MAGIC;
    let size_matches = recorded.is_some_and(|expected| expected == digest.size);
    let head_phrase = if is_key_store { "is" } else { "is not" };
    let detail = if carries && is_key_store && size_matches {
        format!(
            "{} bytes, digest {}; the manifest records {} bytes for the key member and they \
             match; the member's head is a SQLite key-store database",
            digest.size,
            digest.sha256,
            recorded.unwrap_or(0),
        )
    } else if !carries {
        format!(
            "{} bytes, digest {}; the archive carries a key member the manifest does not \
             declare; the member's head {head_phrase} a SQLite key-store database",
            digest.size, digest.sha256,
        )
    } else if !size_matches {
        match recorded {
            Some(expected) => format!(
                "the manifest records {expected} bytes for the key member, the member holds \
                 {}; the member's head {head_phrase} a SQLite key-store database",
                digest.size,
            ),
            None => "manifest field keys.bytes is required when the archive carries a key \
                      member"
                .to_owned(),
        }
    } else {
        format!(
            "{} bytes, digest {}; the member's head {head_phrase} a SQLite key-store database",
            digest.size, digest.sha256,
        )
    };
    Ok(MemberCheck {
        member: KEY_MEMBER.to_owned(),
        ok: carries && is_key_store && size_matches,
        detail,
    })
}

/// A member the manifest does not describe: listed, and never counted as a pass
/// on the manifest's word.
pub(crate) fn unlisted_check(
    reader: &mut ArchiveReader<std::fs::File>,
    name: &str,
) -> Result<MemberCheck, AmError> {
    let size = match reader.skip_current() {
        Ok(size) => size,
        Err(error) => return Err(corrupt(name, &error)),
    };
    Ok(MemberCheck {
        member: name.to_owned(),
        ok: true,
        detail: format!(
            "{size} bytes; the manifest does not describe this member, so it is listed and \
             not trusted"
        ),
    })
}

/// Stream the open payload member into an owner-only staged file.
///
/// The file is adopted only after a clean read and a clean flush. An error on
/// either path removes the file, because an unadopted staged file has no owner
/// that would clean it up.
///
/// **Every failure on this path keeps the code it arrived with** (#83). A
/// staging directory that is read-only, full, or out of names is the machine's
/// condition and is reported as such; re-labelling it `ArchiveCorrupt` told an
/// operator their archive was damaged when the archive was fine and the disk
/// was the problem. Genuine damage still arrives as `ArchiveCorrupt` from the
/// container and still leaves as `ArchiveCorrupt`, because nothing here
/// overwrites it. This is the same rule `extract::staging_failure` applies on
/// the restore path, so the two commands no longer disagree about one
/// filesystem.
pub(crate) fn stage_payload(
    reader: &mut ArchiveReader<std::fs::File>,
    staging_dir: &Path,
    name: &str,
) -> Result<StagedFile, AmError> {
    let (path, mut file) = match staging::begin_staged(staging_dir, "verify-payload") {
        Ok(staged) => staged,
        Err(error) => {
            return Err(staging_failure(
                name,
                "could not be staged for verification",
                &error,
            ));
        }
    };
    let digest = match reader.read_current(&mut file) {
        Ok(digest) => digest,
        Err(error) => {
            // The staged file is not adopted, so nothing would remove it.
            let _ = std::fs::remove_file(&path);
            // A failure to *stream* the member in can be either: the container
            // rejects a short or malformed stream as `ArchiveCorrupt`, and a
            // disk that fills mid-write arrives as the machine's own code.
            // Both keep the code they arrived with.
            return Err(staging_failure(name, "could not be staged", &error));
        }
    };
    if let Err(error) = file.sync_all() {
        let _ = std::fs::remove_file(&path);
        return Err(staging_failure(
            name,
            "could not be flushed after staging",
            &staging::io_error("flushing the payload", &error),
        ));
    }
    Ok(staging::adopt_staged(path, digest.size, digest.sha256))
}
