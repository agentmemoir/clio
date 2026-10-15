// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the durable erasure journal: the owner-only
//! authority that records a denied subject key before its key material is
//! lost.
//!
//! ## Owns
//! - The journal model: schema version, deployment identity, generation,
//!   subject-salt fingerprint, and denials
//! - The journal-state load and the denial, init, and handoff operations
//!
//! ## Does not own
//! - Where the journal lives (`clio-config::resolve_journal_path`) or minting
//!   deployment identities (`clio-config::new_deployment_id`)
//! - The owner-only file mechanics and atomic write (`erasure_journal_file`)
//! - The archive authority check a restore runs (`archive_authority` owns that)
//! - Archive member scanning or restore orchestration (callers own those)
//!
//! ## Boundary
//!
//! Changes to the journal's shape or its deny-or-accept operations should
//! normally require changes here. Changes to where the journal lives or to how
//! a restore is orchestrated should not.
//!
//! Keep this module focused on the durable erasure authority.
//!
//! # Placement and authority limits
//!
//! The caller chooses the journal path; it must sit outside every path a
//! supported restore replaces and outside the managed export staging and
//! cleanup locations. A denial binds the subject key's deployment-wide
//! identity and the confirmed bank scope; it creates no bank-specific key.
//! The generation guard is a guard, not tamper-proofing: an owner-only file
//! cannot detect a same-owner rollback, so an older copy is refused by
//! generation comparison, and the owner-only mode check is a no-op on
//! non-Unix platforms, where no mode bits exist. The journal records the
//! fingerprint of the deployment tombstone salt it was written under; the
//! archive authority check refuses an archive whose fingerprint is absent or
//! differs from it.

use std::fs;
use std::path::Path;

use clio_types::{AmError, ErrorCode};
use serde::{Deserialize, Serialize};

use crate::erasure_journal_file::{ExpectedBody, ReadBody, read_body, write_body};
use crate::export_custody::export_custody_files::io_error;

/// The journal schema version this build writes and accepts.
pub const SCHEMA_VERSION: u32 = 1;

/// One denied subject: the deployment-wide subject key hash and the confirmed
/// bank scope the denial was recorded for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DenialRecord {
    /// The denied subject key's deployment-wide identity.
    pub subject_hash: String,
    /// The confirmed bank scope, sorted and deduplicated; it never creates a bank key.
    pub banks: Vec<String>,
}

/// The durable erasure authority of one deployment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErasureJournal {
    /// Journal schema version; this build accepts [`SCHEMA_VERSION`] only.
    pub schema_version: u32,
    /// The deployment-wide identity this journal belongs to.
    pub deployment_id: String,
    /// Denial generation; it moves by one for every recorded denial.
    pub generation: u64,
    /// The fingerprint of the deployment tombstone salt this journal was written
    /// under, so a restore can refuse an archive whose subject ids were hashed
    /// under a different salt. `None` for a journal written before the field
    /// existed; a reader treats that as unverifiable and fails closed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub salt_fingerprint: Option<String>,
    /// The denials in the order they were recorded.
    pub denials: Vec<DenialRecord>,
}

impl ErasureJournal {
    /// The denial recorded for `subject_hash`, if any.
    #[must_use]
    pub fn denied(&self, subject_hash: &str) -> Option<&DenialRecord> {
        self.denials
            .iter()
            .find(|record| record.subject_hash == subject_hash)
    }

    /// Whether `subject_hash` is currently denied by this journal.
    #[must_use]
    pub fn is_denied(&self, subject_hash: &str) -> bool {
        self.denied(subject_hash).is_some()
    }
}

/// The classification of the owner-only journal path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JournalState {
    /// No file exists at the path.
    Missing,
    /// The path holds something this build will not trust: unparseable,
    /// truncated, unknown schema version, oversized, empty, widened mode,
    /// non-UTF8, trailing bytes, or an IO error.
    Damaged,
    /// A journal this build accepts.
    Current(ErasureJournal),
}

/// Load and classify the journal at `path`.
///
/// Every failure other than "file absent" classifies as [`JournalState::Damaged`];
/// a damaged authority is never reported as current or missing.
#[must_use]
pub fn load_journal(path: &Path) -> JournalState {
    match read_body(path) {
        ReadBody::Absent => JournalState::Missing,
        ReadBody::Damaged => JournalState::Damaged,
        ReadBody::Bytes(bytes) => match parse(&bytes) {
            Some(journal) => JournalState::Current(journal),
            None => JournalState::Damaged,
        },
    }
}

/// Parse a journal body with an explicit schema-version check and no trailing
/// bytes allowed.
fn parse(bytes: &[u8]) -> Option<ErasureJournal> {
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let journal = ErasureJournal::deserialize(&mut deserializer).ok()?;
    deserializer.end().ok()?;
    (journal.schema_version == SCHEMA_VERSION).then_some(journal)
}

/// Encode and durably write `journal` to `path`, refusing when `path` no
/// longer holds the `expected` pre-image (see `write_body`).
fn write_journal(
    path: &Path,
    expected: &ExpectedBody<'_>,
    journal: &ErasureJournal,
) -> Result<(), AmError> {
    let bytes = serde_json::to_vec(journal).map_err(|err| {
        AmError::new(
            ErrorCode::Internal,
            format!("cannot encode the journal: {err}"),
        )
    })?;
    write_body(path, expected, &bytes)
}

/// A fresh empty journal for a new empty deployment.
fn empty_journal() -> ErasureJournal {
    ErasureJournal {
        schema_version: SCHEMA_VERSION,
        deployment_id: clio_config::new_deployment_id(),
        generation: 0,
        salt_fingerprint: Some(crate::erase::tombstone_salt_fingerprint()),
        denials: Vec::new(),
    }
}

/// The refusal for a journal this build will not trust at `path`.
fn damaged_journal(path: &Path, action: &str) -> AmError {
    AmError::new(
        ErrorCode::ConfigCorrupt,
        format!("damaged journal at {}; {action}", path.display()),
    )
}

/// Record a durable denial of `subject_hash` before any key loss, returning
/// the generation the journal carries after the call.
///
/// A missing journal starts a new deployment: a fresh deployment id,
/// generation 0, then this denial, which makes generation 1. A damaged
/// journal refuses; it is never overwritten, because that would drop recorded
/// denials. A current journal that already denies the subject makes no change
/// and returns the current generation, so a recorded scope is never widened.
///
/// # Errors
/// [`ErrorCode::ConfigCorrupt`] when the journal is damaged;
/// [`ErrorCode::OutOfRange`] when the generation is exhausted;
/// [`ErrorCode::Internal`] when the write fails or when the journal changed
/// under the write, which is refused rather than clobbered. The write is
/// durable before this returns.
pub fn record_denial(path: &Path, subject_hash: &str, banks: &[String]) -> Result<u64, AmError> {
    let loaded = read_body(path);
    let (expected, parsed) = match &loaded {
        ReadBody::Bytes(bytes) => (ExpectedBody::Bytes(bytes), parse(bytes)),
        _ => (ExpectedBody::Absent, None),
    };
    let mut journal = match parsed {
        Some(journal) => journal,
        None if matches!(loaded, ReadBody::Absent) => empty_journal(),
        None => return Err(damaged_journal(path, "refusing")),
    };
    if journal.is_denied(subject_hash) {
        return Ok(journal.generation);
    }
    let mut record = DenialRecord {
        subject_hash: subject_hash.to_owned(),
        banks: banks.to_vec(),
    };
    // The recorded bank scope is sorted and deduplicated.
    record.banks.sort();
    record.banks.dedup();
    journal.denials.push(record);
    journal.generation = journal.generation.checked_add(1).ok_or_else(|| {
        AmError::new(
            ErrorCode::OutOfRange,
            "the erasure journal generation is exhausted",
        )
    })?;
    write_journal(path, &expected, &journal)?;
    Ok(journal.generation)
}

/// Create a new empty deployment journal at `path`.
///
/// Refuses when anything already exists at the path — current or damaged —
/// because a new empty journal is valid only for a new empty deployment, and
/// replacing an existing authority would drop its denials. The write refuses
/// when the path appears after that check, so a racing writer is not
/// clobbered. The returned journal carries the base generation, so it
/// certifies no restore by itself.
///
/// # Errors
/// [`ErrorCode::Forbidden`] when the path is occupied; [`ErrorCode::Internal`]
/// when the write fails or refuses a path that appeared under it.
pub fn initialize_new_empty(path: &Path) -> Result<ErasureJournal, AmError> {
    match fs::symlink_metadata(path) {
        Ok(_) => {
            let why = format!("journal already exists at {}", path.display());
            return Err(AmError::new(ErrorCode::Forbidden, why));
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(io_error("inspect", path, &err)),
    }
    let journal = empty_journal();
    write_journal(path, &ExpectedBody::Absent, &journal)?;
    Ok(journal)
}

/// Hand the journal at `source` to `destination` for a new-host restore.
///
/// The source must be current; missing or damaged refuses. A missing
/// destination receives an owner-only copy, which refuses if the destination
/// appeared under it. A destination that is already the same deployment at
/// the same generation is idempotent; anything else at the destination
/// refuses, because another deployment's authority is never overwritten. The
/// destination is re-loaded and its deployment id, generation, and denials
/// are verified before this returns. A trusted operator runs this for a
/// new-host restore; nothing discovers a source.
///
/// # Errors
/// [`ErrorCode::NotFound`] when the source is missing; [`ErrorCode::ConfigCorrupt`]
/// when the source or destination is damaged or the copy fails its reload
/// verification; [`ErrorCode::Forbidden`] when the destination holds another
/// deployment; [`ErrorCode::Internal`] when a write fails or refuses a change
/// under it.
pub fn handoff(source: &Path, destination: &Path) -> Result<ErasureJournal, AmError> {
    let source_journal = match load_journal(source) {
        JournalState::Current(journal) => journal,
        JournalState::Missing => {
            let why = format!("no journal at {} to hand off", source.display());
            return Err(AmError::new(ErrorCode::NotFound, why));
        }
        JournalState::Damaged => return Err(damaged_journal(source, "not handed off")),
    };
    match load_journal(destination) {
        JournalState::Missing => {
            write_journal(destination, &ExpectedBody::Absent, &source_journal)?;
        }
        JournalState::Damaged => return Err(damaged_journal(destination, "refusing")),
        JournalState::Current(existing) => {
            let conflict = if existing.deployment_id != source_journal.deployment_id {
                Some((ErrorCode::Forbidden, "another deployment's authority"))
            } else if existing.generation != source_journal.generation {
                Some((ErrorCode::Forbidden, "a different generation"))
            } else if existing.denials != source_journal.denials {
                Some((ErrorCode::ConfigCorrupt, "different denials"))
            } else {
                None
            };
            if let Some((code, held)) = conflict {
                let why = format!("the destination {} holds {held}", destination.display());
                return Err(AmError::new(code, why));
            }
        }
    }
    match load_journal(destination) {
        JournalState::Current(loaded) if loaded == source_journal => Ok(loaded),
        _ => Err(AmError::new(
            ErrorCode::ConfigCorrupt,
            format!("handoff at {} incomplete", destination.display()),
        )),
    }
}

/// The archive authority check: what an archive claims, what its members show,
/// and the refuse-or-pass rule. It lives in its own module so this one stays
/// focused on the journal.
#[path = "archive_authority.rs"]
mod archive_authority;

pub use archive_authority::{ArchiveClaim, AuthorityRefusal, MemberFacts, check_archive_authority};

#[cfg(test)]
#[path = "erasure_journal_tests.rs"]
mod erasure_journal_tests;

/// The archive authority check tests, split out so the model and operations
/// tests stay focused.
#[cfg(test)]
#[path = "erasure_journal_check_tests.rs"]
mod erasure_journal_check_tests;
