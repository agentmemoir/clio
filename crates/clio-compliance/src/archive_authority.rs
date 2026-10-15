// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the failure-closed authority check a restore
//! runs: what an incoming archive claims about the authority it was made under,
//! what each member shows about subject references, and the refuse-or-pass rule
//! against the live erasure journal.
//!
//! ## Owns
//! - `ArchiveClaim` and `MemberFacts`, the archive's inputs to the check
//! - `AuthorityRefusal` and `check_archive_authority`
//!
//! ## Does not own
//! - The journal model, its operations, and its durable file (`erasure_journal`
//!   owns those)
//! - Reading an archive's members (`clio-backup` and the restore classifier own
//!   that)
//!
//! ## Boundary
//!
//! Changes to the refuse-or-pass rules should normally require changes here.
//! Changes to how the journal is stored or loaded should not.
//!
//! Keep this module focused on the archive authority check.

use super::JournalState;
use clio_types::ErrorCode;

/// What an incoming archive records about the authority it was made under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveClaim {
    /// The deployment identity the archive was made under.
    pub deployment_id: String,
    /// The authority generation the archive was made under.
    pub generation: u64,
    /// The fingerprint of the salt the archive's member subject ids were hashed
    /// under, or `None` for an archive written before the field existed.
    pub salt_fingerprint: Option<String>,
}

/// What one archive member shows about subject references.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemberFacts {
    /// The member proved to hold no subject reference.
    None,
    /// The member references a subject through a content-free record — a
    /// destroyed key slot, a tombstone, or a ciphertext row without a usable
    /// DEK. Ciphertext rows without a usable DEK still count as presence.
    Reference {
        /// The referenced subject key's deployment-wide identity.
        subject_hash: String,
    },
    /// The member cannot be classified.
    Unknown,
}

/// Why the authority check refused a restore.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthorityRefusal {
    /// The live journal is missing.
    CurrentMissing,
    /// The live journal is damaged.
    CurrentDamaged,
    /// No authority was presented with the archive.
    PresentedMissing,
    /// The presented authority is damaged.
    PresentedDamaged,
    /// The presented authority belongs to another deployment.
    PresentedOtherDeployment,
    /// The presented authority is older than the live one: a known-old copy.
    PresentedOlderGeneration,
    /// The presented authority is newer than the live one, so it cannot be the live authority.
    PresentedNewerGeneration,
    /// The archive claims another deployment, so a fresh empty journal cannot validate it.
    ClaimOtherDeployment,
    /// The archive claims a generation newer than the live authority's.
    ClaimNewerGeneration,
    /// The archive's salt fingerprint is absent or differs from the live authority's.
    SaltFingerprintMismatch,
    /// A member cannot be classified, so its subject membership is unknown.
    UnknownMember,
    /// A member references a subject the live authority denies.
    DeniedSubject,
}

impl AuthorityRefusal {
    /// The refusal's stable code in the crate's error vocabulary.
    #[must_use]
    pub const fn code(self) -> ErrorCode {
        match self {
            Self::CurrentMissing | Self::PresentedMissing => ErrorCode::NotFound,
            Self::CurrentDamaged | Self::PresentedDamaged | Self::UnknownMember => {
                ErrorCode::ConfigCorrupt
            }
            Self::PresentedOtherDeployment
            | Self::PresentedOlderGeneration
            | Self::PresentedNewerGeneration
            | Self::ClaimOtherDeployment
            | Self::ClaimNewerGeneration
            | Self::SaltFingerprintMismatch
            | Self::DeniedSubject => ErrorCode::Forbidden,
        }
    }
}

impl std::fmt::Display for AuthorityRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::CurrentMissing => "the erasure journal is missing",
            Self::CurrentDamaged => "the erasure journal is damaged",
            Self::PresentedMissing => "no authority was presented with the archive",
            Self::PresentedDamaged => "the presented authority is damaged",
            Self::PresentedOtherDeployment => "the presented authority is another deployment's",
            Self::PresentedOlderGeneration => "the presented authority is older than the live one",
            Self::PresentedNewerGeneration => "the presented authority is newer than the live one",
            Self::ClaimOtherDeployment => "the archive claims another deployment",
            Self::ClaimNewerGeneration => {
                "the archive claims a generation newer than the live authority's"
            }
            Self::SaltFingerprintMismatch => {
                "the archive's subject-salt fingerprint does not match the live authority's"
            }
            Self::UnknownMember => "an archive member cannot be classified",
            Self::DeniedSubject => "an archive member references a currently denied subject",
        })
    }
}

impl std::error::Error for AuthorityRefusal {}

/// Run the failure-closed authority check for restoring `members` under the
/// live `current` authority, the same authority `presented` with the archive,
/// and what the archive itself `claim`s.
///
/// The check passes only when the live authority is current, the presented
/// authority is that same live authority (same deployment, same generation),
/// the archive claims that deployment at no newer a generation, the archive's
/// subject-salt fingerprint matches the live authority's, and every member
/// either proves no subject reference or references a subject the live
/// authority does not deny. Only proved absence passes; absent, damaged,
/// mismatched, known-older, unknown, or denied material refuses.
///
/// # Errors
/// The [`AuthorityRefusal`] naming the first refusal, in check order.
pub fn check_archive_authority(
    current: &JournalState,
    presented: Option<&JournalState>,
    claim: &ArchiveClaim,
    members: &[MemberFacts],
) -> Result<(), AuthorityRefusal> {
    let current = match current {
        JournalState::Current(journal) => journal,
        JournalState::Missing => return Err(AuthorityRefusal::CurrentMissing),
        JournalState::Damaged => return Err(AuthorityRefusal::CurrentDamaged),
    };
    let presented = match presented {
        Some(JournalState::Current(journal)) => journal,
        Some(JournalState::Damaged) => return Err(AuthorityRefusal::PresentedDamaged),
        None | Some(JournalState::Missing) => return Err(AuthorityRefusal::PresentedMissing),
    };
    if presented.deployment_id != current.deployment_id {
        return Err(AuthorityRefusal::PresentedOtherDeployment);
    }
    if presented.generation < current.generation {
        return Err(AuthorityRefusal::PresentedOlderGeneration);
    }
    if presented.generation > current.generation {
        return Err(AuthorityRefusal::PresentedNewerGeneration);
    }
    if claim.deployment_id != current.deployment_id {
        return Err(AuthorityRefusal::ClaimOtherDeployment);
    }
    if claim.generation > current.generation {
        return Err(AuthorityRefusal::ClaimNewerGeneration);
    }
    // The salt that hashed the archive's subject ids must be the salt the live
    // authority recorded, or the recorded hashes are not comparable. An absent
    // fingerprint on either side fails closed.
    if claim.salt_fingerprint.is_none() || claim.salt_fingerprint != current.salt_fingerprint {
        return Err(AuthorityRefusal::SaltFingerprintMismatch);
    }
    for member in members {
        match member {
            MemberFacts::None => {}
            MemberFacts::Unknown => return Err(AuthorityRefusal::UnknownMember),
            MemberFacts::Reference { subject_hash } => {
                if current.is_denied(subject_hash) {
                    return Err(AuthorityRefusal::DeniedSubject);
                }
            }
        }
    }
    Ok(())
}
