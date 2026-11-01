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
//!   path this deployment resolves
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
use clio_types::AmError;

use crate::erasure_journal::{JournalState, load_journal, record_denial};
use crate::restore_guard::JournalHold;
use crate::salt::{tombstone_salt_at, tombstone_salt_fingerprint_of};

/// The erasure-denial seam over the owner-only erasure journal at one path.
pub struct JournalDenialProbe {
    journal_path: PathBuf,
    salt: Vec<u8>,
    salt_fingerprint: String,
}

impl JournalDenialProbe {
    /// A probe for the journal at `journal_path`.
    ///
    /// # Errors
    /// The same failures the deployment salt beside that path reports.
    pub fn new(journal_path: PathBuf) -> Result<Self, AmError> {
        let salt = tombstone_salt_at(&journal_path)?;
        Ok(Self {
            journal_path,
            salt_fingerprint: tombstone_salt_fingerprint_of(&salt),
            salt,
        })
    }

    /// The journal path this probe reads and writes.
    #[must_use]
    pub fn journal_path(&self) -> &Path {
        &self.journal_path
    }

    /// The deployment-wide identity the journal stores for `subject_id`.
    ///
    /// The hash is the same one the erasure path records and the tombstone
    /// hashes carry, so the probe answers about the identity the rest of the
    /// deployment uses.
    fn subject_hash(&self, subject_id: &str) -> String {
        sha256_hex(&self.salt, subject_id)
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
    /// authority and must refuse its action rather than run unguarded.
    ///
    /// # Errors
    /// `ConfigCorrupt` when no path resolves, or when a document-named path
    /// differs from the deployment resolution, plus the failures the deployment
    /// salt beside the resolved path reports.
    pub fn resolved(
        document: Option<&Path>,
        get: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Self, AmError> {
        Self::new(JournalHold::resolve_journal_path(document, get)?)
    }
}

impl ErasureDenial for JournalDenialProbe {
    fn verdict(&self, subject_id: &str) -> DenialVerdict {
        match load_journal(&self.journal_path) {
            JournalState::Current(journal) if journal.is_denied(&self.subject_hash(subject_id)) => {
                DenialVerdict::Denied
            }
            JournalState::Current(_) | JournalState::Missing => DenialVerdict::Clear,
            JournalState::Damaged => DenialVerdict::AuthorityUnavailable,
        }
    }

    fn record_denial(&self, subject_id: &str, banks: &[String]) -> Result<u64, AmError> {
        record_denial(&self.journal_path, &self.subject_hash(subject_id), banks)
    }

    fn salt_fingerprint(&self) -> String {
        self.salt_fingerprint.clone()
    }
}

#[cfg(test)]
#[path = "journal_denial_tests.rs"]
mod journal_denial_tests;
