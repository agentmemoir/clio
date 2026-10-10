// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the space and placement checks a restore runs
//! before it writes anything: proving there is room for the payload, the key
//! member, and the pre-restore snapshot, and deciding where the archive's key
//! member goes.
//!
//! ## Owns
//! - `preflight`, the three space footprints, what each one covers, and the
//!   rule that footprints sharing a directory are checked as that directory's
//!   whole total
//! - `key_destination`, the key placement and its "nowhere to live" refusal
//!
//! ## Does not own
//! - The plan order and the per-backend plan (see `backup_restore_flow`)
//! - The space check and the file swap (see `clio-store`'s restore pre-flight
//!   and `restore_swap`)
//!
//! ## Boundary
//!
//! Changes to what the restore reserves room for, or to where a key member
//! lands, should normally require changes here. Changes to the report shape or
//! the exit codes should not.
//!
//! Keep this module focused on the pre-write space and key checks.

use crate::backup_cli::ResolvedTarget;
use crate::backup_restore_exit::{self, RestoreFailure};
use crate::cli_error::CliError;
use clio_store::{KeyPlacement, SpaceCheck, SpaceFootprint};
use std::path::Path;

/// Prove room for the restored payload, the archive's key member, and the
/// pre-restore snapshot, before the snapshot is taken.
///
/// **What these footprints cover.** There are three of them. The first covers
/// the payload member. The second covers the key member, and is pushed only when
/// the manifest records a key size; `key_bytes` is that size. The third covers
/// the pre-restore snapshot in the backup directory. A SQLite restore's first
/// two land in the target's own directory and its third in the backup
/// directory; a Postgres restore has all three in the backup directory, because
/// the dump, the key member, and the snapshot are all extracted or written
/// there.
///
/// **Footprints that share a directory are checked as that directory's total,
/// not one at a time.** The extractor stages the payload and the key member side
/// by side, and both exist on disk until the swap, so the sum is what the
/// directory has to hold. Asking per footprint would let two members each fit
/// under the free space while the pair does not, and the second one would then
/// fail to stage after the pre-flight had said yes.
///
/// **What they do not cover:** an archive written before the manifest recorded a
/// key size. Such an archive reads as `key_bytes == None`, no key footprint is
/// reserved, and the run can still fail while staging a key member larger than
/// the room the other two leave. That is the honest limit of a pre-flight
/// against an archive that does not say: the number exists only where the
/// manifest carries it.
///
/// So the guarantee is exactly this: a refusal here leaves nothing behind,
/// because the snapshot has not been taken and no staged member exists yet. It
/// is a promise that every byte the manifest can account for was counted, and
/// nothing wider than that.
///
/// A directory with no room is an operational failure (exit 1) and a directory
/// that will not accept writes is a refusal (exit 2); the exit-code table decides
/// both, at plan time and at place time alike.
pub(crate) fn preflight(
    target_dir: &Path,
    backup_dir: &Path,
    payload_bytes: u64,
    key_bytes: Option<u64>,
    snapshot_bytes: u64,
    check: &dyn SpaceCheck,
) -> Result<(), CliError> {
    let mut footprints = vec![SpaceFootprint {
        dir: target_dir.to_path_buf(),
        bytes: payload_bytes,
        label: "the restored payload",
    }];
    if let Some(bytes) = key_bytes {
        footprints.push(SpaceFootprint {
            dir: target_dir.to_path_buf(),
            bytes,
            label: "the archive's key member",
        });
    }
    footprints.push(SpaceFootprint {
        dir: backup_dir.to_path_buf(),
        bytes: snapshot_bytes,
        label: "the pre-restore snapshot",
    });
    clio_store::preflight_restore(&footprints, check).map_err(|error| {
        backup_restore_exit::from_am_error(backup_restore_exit::space_failure(&error), &error)
    })
}

/// Where the archive's key member goes, refusing a key with nowhere to live.
pub(crate) fn key_destination(
    staged_key: Option<&Path>,
    target: &ResolvedTarget,
) -> Result<Option<KeyPlacement>, CliError> {
    let Some(staged) = staged_key else {
        return Ok(None);
    };
    let Some(destination) = target.kms_path.clone() else {
        let hint = if target.is_postgres() {
            "a Postgres key store has no file path; restore into a target this build created"
        } else {
            "point `--db` at a file-backed SQLite store so its key store has a path"
        };
        return Err(backup_restore_exit::error(
            RestoreFailure::KeyNowhereToLive,
            "missing_key_store",
            format!(
                "the archive carries a key member, but there is no key-store path to place it \
                 at for {}",
                target.display
            ),
        )
        .with_hint(hint));
    };
    Ok(Some(KeyPlacement {
        staged: staged.to_path_buf(),
        destination,
    }))
}

/// The key placement, and its refusals, for a target with this key-store path.
#[cfg(test)]
#[path = "backup_restore_space_tests.rs"]
mod tests;
