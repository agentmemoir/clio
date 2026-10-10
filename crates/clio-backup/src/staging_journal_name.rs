// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for recognising a name in the staging area: the
//! name family a staged member and a journal share, and the two rules that
//! decide whether a name is a journal this sweep may read or a staged file it
//! may remove.
//!
//! ## Owns
//! - The `clio-snapshot-` name family: the prefix, the journal tag, and the
//!   two suffixes
//! - `is_journal_name`: the exact shape a journal is written in, so only a
//!   journal this module's owner wrote is read or removed
//! - `is_staged_name`: the shape a staged member is named in, plus the
//!   one-component rule that keeps a name inside the directory being swept
//!
//! ## Does not own
//! - Writing a journal, naming one, or removing one (see `staging_journal`)
//! - The name `staging::reserve_path` hands out (see `staging`)
//! - Deciding whether a recorded owner is still running (see `staging_journal`)
//!
//! ## Boundary
//!
//! Changes to the name family, or to what the name of a journal or of a staged
//! member has to look like, should normally require changes here. Changes to
//! what a restore records, or to how a staged file is written and removed,
//! should not require changes here.
//!
//! Keep this module focused on recognising a name, never on acting on one.

use std::ffi::OsStr;
use std::path::Path;

/// The name family a staged member and a journal share, so a directory listing
/// explains itself and the sweep can recognise what it may delete.
pub(crate) const STAGING_PREFIX: &str = "clio-snapshot-";

/// The tag this module's own journal carries in its name.
pub(crate) const JOURNAL_TAG: &str = "restore-journal";

/// A journal's own suffix. No staged member has it, so the two can never be
/// mistaken for each other in either direction.
pub(crate) const JOURNAL_SUFFIX: &str = ".journal";

/// The suffix a staged member's name ends in.
pub(crate) const STAGED_SUFFIX: &str = ".tmp";

/// Whether `name` is a journal this sweep may read.
///
/// The shape is the one a journal is written in, field for field: the family
/// prefix, the journal tag, a pid, a nanosecond stamp, an attempt number, and
/// the journal suffix. A file that does not match was not written by a restore
/// of this shape, so it is neither read nor deleted, whatever a directory
/// listing of somebody else's files happens to contain.
///
/// The tag carries a dash of its own, so the split is `rsplitn(4, '-')`: the
/// three numbers, then everything in front of them, which has to be the tag
/// itself. Anything longer in front of the numbers — a second tag, a stray
/// field — makes that remainder something the tag is not.
pub(crate) fn is_journal_name(name: &str) -> bool {
    let Some(stem) = name
        .strip_prefix(STAGING_PREFIX)
        .and_then(|rest| rest.strip_suffix(JOURNAL_SUFFIX))
    else {
        return false;
    };
    let mut fields = stem.rsplitn(4, '-');
    let attempt = fields.next().unwrap_or_default();
    let nanos = fields.next().unwrap_or_default();
    let pid = fields.next().unwrap_or_default();
    // `create` stamps the elapsed time in nanoseconds, which is a u128, so that
    // is what a real name's middle field has to parse as.
    attempt.parse::<u32>().is_ok()
        && nanos.parse::<u128>().is_ok()
        && pid.parse::<u32>().is_ok()
        && fields.next() == Some(JOURNAL_TAG)
}

/// Whether `name` is a staged member's own name, and so something this sweep
/// may delete.
///
/// The name `staging::reserve_path` hands out is
/// `clio-snapshot-<tag>-<pid>-<nanos>-<attempt>.tmp`. Both the family prefix and
/// the trailing `.tmp` are required, so a journal entry cannot name an archive,
/// a manifest, or a journal and have it removed; and the two fields before the
/// attempt must be numbers, so a name that merely starts with the family's
/// prefix is not swept either.
///
/// A name is also required to be one leaf: `file_name` hands back the whole
/// name only when there is no `/` and no `.` or `..` to resolve, so this rejects
/// every name that is a path rather than a file, and the sweep never joins a
/// name that could leave the directory it is sweeping.
pub(crate) fn is_staged_name(name: &str) -> bool {
    if Path::new(name).file_name() != Some(OsStr::new(name)) {
        return false;
    }
    let Some(stem) = name
        .strip_prefix(STAGING_PREFIX)
        .and_then(|rest| rest.strip_suffix(STAGED_SUFFIX))
    else {
        return false;
    };
    let mut fields = stem.rsplitn(4, '-');
    let _attempt = fields.next();
    let nanos = fields.next().unwrap_or_default();
    let pid = fields.next().unwrap_or_default();
    nanos.parse::<u64>().is_ok() && pid.parse::<u32>().is_ok()
}
