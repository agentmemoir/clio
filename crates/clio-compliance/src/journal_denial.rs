// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the production erasure-denial seam: the
//! owner-only erasure journal at one path, exposed to a caller that cannot
//! depend on this crate.
//!
//! ## Owns
//! - `JournalDenialProbe` and its `ErasureDenial` implementation: the verdict
//!   classification, the durable denial write, and the salt and fingerprint it
//!   answers under
//! - `resolved`: the fail-closed construction of the probe from the journal
//!   path this deployment resolves, and `new`: the test and in-process
//!   constructor, which holds no salt when none resolves and fails closed
//!
//! ## Does not own
//! - The journal model, its load classification, and the denial operation
//!   (`erasure_journal` owns those; this module only wires them)
//! - Resolving the deployment salt and its fingerprint (`salt` owns those; the
//!   probe only holds the values resolved once at construction)
//! - The resolution rule choosing that path (`restore_guard` and `clio-config`
//!   own it; `resolved` only requires it to succeed), and the owner-only file
//!   mechanics (`erasure_journal_file`)
//!
//! ## Boundary
//!
//! Changes to how the journal answers the seam should normally require
//! changes here. Changes to the journal's own model, checks, or file
//! mechanics should not.
//!
//! Keep this module focused on adapting the journal to the seam.

use std::path::{Path, PathBuf};

use clio_store::{DenialVerdict, ErasureDenial, sha256_hex};
use clio_types::{AmError, ErrorCode};

use crate::erasure_journal::{ErasureJournal, JournalState, load_journal, record_denial};
use crate::restore_guard::JournalHold;
use crate::salt::{tombstone_salt_at, tombstone_salt_fingerprint_of};

/// The erasure-denial seam over the owner-only erasure journal at one path.
pub struct JournalDenialProbe {
    journal_path: PathBuf,
    /// The deployment salt resolved once at construction, or `None` when this
    /// process could not resolve one beside the journal.
    ///
    /// Absent never means "hash under nothing": every answer below fails closed
    /// instead, so no subject identity is ever derived from an empty salt and
    /// no fingerprint is ever invented.
    salt: Option<Vec<u8>>,
    /// The fingerprint of [`Self::salt`], `None` exactly when it is absent.
    salt_fingerprint: Option<String>,
}

impl JournalDenialProbe {
    /// A probe for the journal at `journal_path`.
    ///
    /// This is the test and in-process constructor: it never refuses, so a
    /// journal this process cannot resolve a deployment salt for still yields a
    /// probe. That probe fails closed rather than hashing under an empty or
    /// fabricated salt — [`ErasureDenial::verdict`] answers
    /// [`DenialVerdict::AuthorityUnavailable`],
    /// [`ErasureDenial::record_denial`] refuses with `ConfigCorrupt` naming the
    /// missing deployment salt, and [`ErasureDenial::salt_fingerprint`] is blank,
    /// which its callers already refuse.
    ///
    /// Production does not build a probe this way:
    /// [`JournalDenialProbe::resolved`] is the only constructor the deployment
    /// paths call, and it propagates a salt-resolution failure instead of
    /// holding a fail-closed probe.
    #[must_use]
    pub fn new(journal_path: PathBuf) -> Self {
        match tombstone_salt_at(&journal_path) {
            Ok(salt) => Self::under_salt(journal_path, salt),
            Err(_) => Self {
                journal_path,
                salt: None,
                salt_fingerprint: None,
            },
        }
    }

    /// A probe whose deployment salt is already resolved.
    fn under_salt(journal_path: PathBuf, salt: Vec<u8>) -> Self {
        Self {
            journal_path,
            salt_fingerprint: Some(tombstone_salt_fingerprint_of(&salt)),
            salt: Some(salt),
        }
    }

    /// The journal path this probe reads and writes.
    #[must_use]
    pub fn journal_path(&self) -> &Path {
        &self.journal_path
    }

    /// The deployment-wide identity the journal stores for `subject_id`, or
    /// `None` when no deployment salt resolved so there is nothing honest to
    /// hash under.
    ///
    /// The hash is the same one the erasure path records and the tombstone
    /// hashes carry, so the probe answers about the identity the rest of the
    /// deployment uses.
    fn subject_hash(&self, subject_id: &str) -> Option<String> {
        self.salt
            .as_deref()
            .map(|salt| sha256_hex(salt, subject_id))
    }

    /// The refusal a probe with no resolved deployment salt answers a write
    /// with: a denial that names no salt is not a durable denial.
    fn missing_salt(&self) -> AmError {
        AmError::new(
            ErrorCode::ConfigCorrupt,
            format!(
                "the deployment salt beside the erasure journal {} could not be resolved, so this \
                 probe cannot record a denial: a subject hashed under an empty salt would not be \
                 a durable denial",
                self.journal_path.display()
            ),
        )
    }

    /// Whether `journal` was recorded under a salt other than the one this probe
    /// holds.
    ///
    /// A journal with no recorded fingerprint cannot be verified against any
    /// salt, so it counts as another salt: an unverifiable journal is never
    /// read as one this deployment's salt answers for.
    fn under_another_salt(&self, journal: &ErasureJournal) -> bool {
        journal.salt_fingerprint.as_deref() != self.salt_fingerprint.as_deref()
    }

    /// The probe for the journal this deployment resolves, or a refusal.
    ///
    /// `document` is the erasure-journal path a host-supplied effective document
    /// named, when the host resolved one; `None` keeps the deployment resolution
    /// (the `CLIO_ERASURE_JOURNAL_FILE` override over the resolved data
    /// directory). Resolution is [`JournalHold::resolve_journal_path`], the one
    /// rule the erase boundary applies, so the denials a local erase records are
    /// the denials this probe answers with.
    ///
    /// There is no fallback: a route that cannot name its journal holds no
    /// authority and must refuse its action rather than run unguarded. The
    /// deployment salt is resolved here and a failure propagates, so production
    /// never degrades to a probe that can answer nothing.
    ///
    /// # Errors
    /// `ConfigCorrupt` when no path resolves, or when a document-named path
    /// differs from the deployment resolution, plus the failures the deployment
    /// salt beside the resolved path reports.
    pub fn resolved(
        document: Option<&Path>,
        get: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Self, AmError> {
        let journal_path = JournalHold::resolve_journal_path(document, get)?;
        let salt = tombstone_salt_at(&journal_path)?;
        Ok(Self::under_salt(journal_path, salt))
    }
}

impl ErasureDenial for JournalDenialProbe {
    fn verdict(&self, subject_id: &str) -> DenialVerdict {
        // No salt resolved, so no identity can be derived: the authority is
        // unavailable and the caller refuses, rather than any subject reading
        // as clear.
        let Some(hash) = self.subject_hash(subject_id) else {
            return DenialVerdict::AuthorityUnavailable;
        };
        match load_journal(&self.journal_path) {
            // A journal recorded under another salt cannot be compared with this
            // probe's salt: the denials it holds were hashed under a salt this
            // process does not hold, so a hash that does not match here is no
            // evidence that the subject is undenied. Unverifiable is never
            // reported as clear.
            JournalState::Current(journal) if self.under_another_salt(&journal) => {
                DenialVerdict::AuthorityUnavailable
            }
            JournalState::Current(journal) if journal.is_denied(&hash) => DenialVerdict::Denied,
            JournalState::Current(_) | JournalState::Missing => DenialVerdict::Clear,
            JournalState::Damaged => DenialVerdict::AuthorityUnavailable,
        }
    }

    fn record_denial(&self, subject_id: &str, banks: &[String]) -> Result<u64, AmError> {
        let hash = self
            .subject_hash(subject_id)
            .ok_or_else(|| self.missing_salt())?;
        record_denial(&self.journal_path, &hash, banks)
    }

    fn salt_fingerprint(&self) -> String {
        // A blank fingerprint is the signal a caller refuses on; it is never
        // filled in with a fabricated value.
        self.salt_fingerprint.clone().unwrap_or_default()
    }
}

#[cfg(test)]
#[path = "journal_denial_tests.rs"]
mod journal_denial_tests;
