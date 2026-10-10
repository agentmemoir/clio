// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for one restore's schema gate: comparing the
//! schema version an archive records against the one this build opens, and
//! refusing an archive that is newer than the build can read.
//!
//! ## Owns
//! - `Schema`, the comparison's result, and the three actions it can name
//! - `compare_schema`, the gate itself
//!
//! ## Does not own
//! - The other restore refusals (see `backup_restore_flow`)
//! - What the store does with an older schema on first open (see `clio-store`)
//!
//! ## Boundary
//!
//! Changes to how an archive's schema version is judged should normally require
//! changes here. Changes to the order a restore runs in should not.
//!
//! Keep this module focused on the schema gate.

use crate::cli_error::CliError;

pub(crate) const ACTION_NONE: &str = "none";
pub(crate) const ACTION_MIGRATIONS: &str = "migrations will run on first open";
pub(crate) const ACTION_UNKNOWN: &str = "unknown";

/// The archive's schema version against this build's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Schema {
    /// The archive's recorded version; `None` when the manifest records none.
    pub(crate) archive: Option<String>,
    /// `none`, `migrations will run on first open`, or `unknown`.
    pub(crate) action: &'static str,
}

/// Compare the archive's schema version with this build's: a newer archive is
/// refused, and an unreadable version is unknown rather than guessed at.
pub(crate) fn compare_schema(archive: Option<&str>, build: &str) -> Result<Schema, CliError> {
    let Some(raw) = archive else {
        return Ok(Schema {
            archive: None,
            action: ACTION_UNKNOWN,
        });
    };
    let (Ok(archive_version), Ok(build_version)) = (raw.parse::<u64>(), build.parse::<u64>())
    else {
        return Ok(Schema {
            archive: Some(raw.to_owned()),
            action: ACTION_UNKNOWN,
        });
    };
    if archive_version > build_version {
        return Err(CliError::refusal(
            "schema_newer",
            format!(
                "the archive records store schema {raw}, but this build's store schema is {build}; \
                 the archive is newer than this build can open"
            ),
        )
        .with_hint(format!(
            "upgrade clio, or restore an archive whose schema is {build} or older"
        )));
    }
    Ok(Schema {
        archive: Some(raw.to_owned()),
        action: if archive_version == build_version {
            ACTION_NONE
        } else {
            ACTION_MIGRATIONS
        },
    })
}
