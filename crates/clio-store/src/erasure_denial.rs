// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the erasure-denial seam: the interface a
//! caller uses to ask one deployment's denial authority about a subject, to
//! record a durable denial, and to name the deployment salt the answer came
//! from — without depending on the authority's implementation.
//!
//! ## Owns
//! - `ErasureDenial` and the three-way verdict type
//!
//! ## Does not own
//! - Any implementation: the production authority is the owner-only erasure
//!   journal, which lives outside this crate and implements this seam
//! - The durable journal format, its generation, and its file mechanics
//! - Which action a caller refuses, and the refusal rule it applies to a
//!   verdict (the caller owns both)
//!
//! ## Boundary
//!
//! Changes to what a caller can ask the denial authority should normally
//! require changes here. Changes to how the authority records a denial
//! should not require changes here.
//!
//! Keep this module focused on the denial seam.

use clio_types::AmError;

/// The answer to "is this subject denied?" from one deployment's authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DenialVerdict {
    /// A current authority records the denial.
    Denied,
    /// No denial is recorded. An absent authority — a fresh deployment with
    /// no journal file yet — is clear, not unavailable.
    Clear,
    /// The authority exists but cannot be read or trusted. A caller must
    /// refuse the affected action instead of treating this as clear.
    AuthorityUnavailable,
}

/// The local erasure-denial authority, as this crate can depend on it.
///
/// This is a seam: the caller holds one of these and never the authority's
/// implementation. Implementations MUST report an unreadable authority as
/// [`DenialVerdict::AuthorityUnavailable`] — never as [`DenialVerdict::Clear`]
/// — and MUST treat an absent authority as clear, so a fresh deployment stays
/// usable. A repeated denial of an already-denied subject MUST make no change
/// and return the current generation, so a redelivered operation cannot
/// double-apply.
pub trait ErasureDenial: Send + Sync {
    /// The verdict for one subject, named by its subject id.
    ///
    /// The implementation derives the deployment-wide identity from the id
    /// under the deployment salt [`ErasureDenial::salt_fingerprint`] names.
    fn verdict(&self, subject_id: &str) -> DenialVerdict;

    /// Durably record the denial of `subject_id` for `banks`, returning the
    /// generation the authority carries after the call — the durable proof a
    /// caller records for the peer that delivered the operation.
    ///
    /// # Errors
    /// A damaged or unreadable authority, an exhausted generation, or a
    /// write failure. The write is durable before this returns.
    fn record_denial(&self, subject_id: &str, banks: &[String]) -> Result<u64, AmError>;

    /// The fingerprint of the deployment salt this authority hashes subject
    /// identities under, so a peer can prove which deployment answered.
    fn salt_fingerprint(&self) -> String;
}

/// The deployment fingerprint the seam tests name. Test-only: one fresh
/// value per process, so no fixed salt bytes appear in the tests.
#[cfg(test)]
pub(crate) fn test_salt_fingerprint() -> &'static str {
    static FP: std::sync::LazyLock<String> =
        std::sync::LazyLock::new(|| rand::random::<u64>().to_string());
    FP.as_str()
}

#[cfg(test)]
#[path = "erasure_denial_tests.rs"]
mod erasure_denial_tests;
