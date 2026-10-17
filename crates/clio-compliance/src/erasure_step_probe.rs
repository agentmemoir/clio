// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the durable step probes of a confirmed
//! erasure: the per-step state a retry starts from and the reads that decide
//! which destructive steps are still outstanding.
//!
//! ## Owns
//! - `ErasureStepStatus`, the durable step state of one confirmed erasure
//! - `probe_erasure_steps`, the durable negative-DEK and tombstone probes
//!
//! ## Does not own
//! - The retry flow that consumes the probe (`erasure_retry.rs`)
//! - The store's tombstone and DEK reads and their SQL (`clio-store`)
//! - The erasure workflow (`erase.rs`)
//!
//! ## Boundary
//!
//! Changes to what a retry probes, or to how it reads it, should normally
//! require changes here. Changes to the retry flow or the erasure workflow
//! should not.
//!
//! Keep this module focused on the erasure step probes.

use clio_store::{ComplianceStore, sha256_hex};
use clio_types::AmError;

/// The durable step state of one confirmed erasure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ErasureStepStatus {
    /// The subject's content key has been destroyed; this step is irreversible.
    pub(crate) key_destruction_done: bool,
    /// The store work committed: this request's erasure tombstones are
    /// present, so the erase transaction that wrote them committed.
    pub(crate) sql_work_done: bool,
    /// The committed purge names the probed subject: one of the request's
    /// tombstones carries its one-way subject hash. Meaningful only when
    /// `sql_work_done` is true; a purge that names only other subjects is
    /// another subject's committed work.
    pub(crate) purge_names_subject: bool,
}

impl ErasureStepStatus {
    /// True when the destructive work is done and only the derived refresh
    /// remains before the settle.
    #[must_use]
    pub(crate) fn only_refresh_remains(self) -> bool {
        self.key_destruction_done && self.sql_work_done
    }
}

/// Read the durable step state a retry starts from.
///
/// The key probe is the store's negative DEK probe. The store-work probe reads
/// the committed fact: the request's erasure tombstones were written inside
/// the erase transaction, just before it commits, so a rolled-back attempt
/// left none however far it got, and present tombstones mean the purge
/// committed. Each tombstone carries the one-way hash of the subject its purge
/// ran for, so the same read reports whether the evidence names the probed
/// subject. A probe failure propagates, so an unknown state never skips work.
///
/// # Errors
/// A store probe failure.
pub(crate) fn probe_erasure_steps(
    store: &dyn ComplianceStore,
    bank_id: &str,
    subject_id: &str,
    request_id: &str,
    salt: &[u8],
) -> Result<ErasureStepStatus, AmError> {
    let key_destruction_done = store.subject_dek_destroyed(subject_id)?;
    let tombstones = store.read_tombstones(bank_id, request_id)?;
    let subject_hash = sha256_hex(salt, subject_id);
    Ok(ErasureStepStatus {
        key_destruction_done,
        sql_work_done: !tombstones.is_empty(),
        purge_names_subject: tombstones
            .iter()
            .any(|tombstone| tombstone.subject_hash == subject_hash),
    })
}
