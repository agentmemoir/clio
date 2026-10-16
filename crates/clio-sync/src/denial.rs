// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the denial operation's wire vocabulary: the
//! stable idempotency key and content of the one sync mutation a denial
//! travels as, the durable proof a peer answers a denial with, and the
//! reserved bank markers that carry a bank's serve answer.
//!
//! ## Owns
//! - `DenialCarrier` (the request reference and the covered banks) and its
//!   decode and validation
//! - The stable denial mutation id convention
//! - `DenialProof`, `DenialProofBinding`, and the fail-closed verification
//! - The reserved denied-bank marker id namespace
//!
//! ## Does not own
//! - The authority-facing guards that record a denial, refuse a mutation, or
//!   answer a served bank (see `denial_guard`)
//! - Applying a mutation (see `apply`) or serving a pull (see `server`)
//! - The authority itself: the `ErasureDenial` seam and the journal behind it
//!   live outside this crate
//!
//! ## Boundary
//!
//! Changes to what one denial operation carries on the wire, or to what a
//! proof binds, should normally require changes here. Changes to how a denial
//! is recorded or refused should not.
//!
//! Keep this module focused on the denial operation's wire vocabulary.

use clio_store::{DenialVerdict, ErasureDenial};
use clio_types::{AmError, ErrorCode};
use serde::{Deserialize, Serialize};

use crate::protocol::{ENTITY_KIND_TOMBSTONE, OP_UPSERT, SyncContent, SyncMutation};

/// The action kind a denial acknowledgement answers for.
pub const DENIAL_ACTION: &str = "denial";

/// The reserved subject-id prefix one denied bank is recorded under.
///
/// The prefix is reserved: a marker id never names a data subject, and a
/// subject id that happened to begin with it would only ever make a bank
/// answer fail closed.
pub const DENIED_BANK_MARKER_PREFIX: &str = "clio:denied-bank:";

/// The reserved subject id the serve guard for `bank` is recorded under.
///
/// The authority answers per subject id and a journal row carries no readable
/// subject, so the durable answer to "is this bank denied" is one more record
/// in that same authority, keyed by this reserved id.
#[must_use]
pub fn denied_bank_marker(bank: &str) -> String {
    format!("{DENIED_BANK_MARKER_PREFIX}{bank}")
}

/// The stable sync mutation id one denial operation carries.
///
/// Derived from the durable request reference alone, so a retry of the same
/// operation reuses exactly one journal idempotency key and can never apply a
/// second time.
#[must_use]
pub fn denial_mutation_id(request_ref: &str) -> String {
    format!("denial:{request_ref}")
}

/// The content of the one sync mutation a denial travels as.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DenialCarrier {
    /// The durable request reference the denial operates under.
    pub request_ref: String,
    /// The banks the request coordinates. The denial covers every one of them.
    pub banks: Vec<String>,
}

impl DenialCarrier {
    /// A carrier for `request_ref` covering `banks`.
    #[must_use]
    pub fn new(request_ref: impl Into<String>, banks: &[&str]) -> Self {
        Self {
            request_ref: request_ref.into(),
            banks: banks.iter().map(|bank| (*bank).to_owned()).collect(),
        }
    }

    /// Validate the carrier shape.
    ///
    /// # Errors
    /// A blank request reference, no covered bank, or a blank bank name.
    pub fn validate(&self) -> Result<(), AmError> {
        if self.request_ref.trim().is_empty() {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "a denial carrier requires a request reference",
            ));
        }
        if self.banks.is_empty() {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "a denial carrier requires at least one covered bank",
            ));
        }
        if self.banks.iter().any(|bank| bank.trim().is_empty()) {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "a denial carrier bank cannot be blank",
            ));
        }
        Ok(())
    }

    /// The plaintext content this carrier travels as.
    #[must_use]
    pub fn content(&self) -> SyncContent {
        SyncContent::Plain(serde_json::to_value(self).expect("serialize denial carrier"))
    }

    /// The covered banks, sorted and deduplicated, so one bank costs one
    /// serve-guard record whatever order the carrier listed it in.
    #[must_use]
    pub fn covered_banks(&self) -> Vec<String> {
        let mut banks = self.banks.clone();
        banks.sort();
        banks.dedup();
        banks
    }
}

/// The carrier one mutation decodes to; `None` when the mutation is not a
/// denial operation at all.
///
/// # Errors
/// A denial operation whose content is not plaintext, or whose content does
/// not carry a valid request reference and covered banks. A malformed carrier
/// is refused, never silently ignored.
pub fn denial_carrier(m: &SyncMutation) -> Result<Option<DenialCarrier>, AmError> {
    if m.entity_kind != ENTITY_KIND_TOMBSTONE || m.op != OP_UPSERT {
        return Ok(None);
    }
    let SyncContent::Plain(value) = &m.content else {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "a denial carrier must travel as plaintext content",
        ));
    };
    let carrier: DenialCarrier = serde_json::from_value(value.clone()).map_err(|e| {
        AmError::new(
            ErrorCode::InvalidArgument,
            format!("bad denial carrier content: {e}"),
        )
    })?;
    carrier.validate()?;
    Ok(Some(carrier))
}

/// Build the one mutation a denial operation travels as.
///
/// The mutation id is [`denial_mutation_id`], so every retry of the same
/// request reuses exactly one journal idempotency key. The content is the
/// carrier, and its digest follows the crate's plaintext content-digest
/// convention.
#[must_use]
pub fn denial_mutation(
    bank: &str,
    subject_id: &str,
    carrier: &DenialCarrier,
    device_id: &str,
    transaction_time: &str,
) -> SyncMutation {
    let value = serde_json::to_value(carrier).expect("serialize denial carrier");
    let text = serde_json::to_string(&value).expect("serialize denial carrier");
    SyncMutation {
        sync_mutation_id: denial_mutation_id(&carrier.request_ref),
        bank: bank.to_owned(),
        entity_kind: ENTITY_KIND_TOMBSTONE.into(),
        entity_id: subject_id.to_owned(),
        op: OP_UPSERT.into(),
        transaction_time: transaction_time.to_owned(),
        device_id: device_id.to_owned(),
        content: SyncContent::Plain(value),
        content_hash: crate::keyed_roles::content_digest(None, &text),
        admission_receipt: None,
    }
}

/// The content-free digest a denial proof binds the subject with.
///
/// Keyed by the answering deployment's published salt fingerprint, so the
/// proof names an identity without carrying the subject id, and both sides can
/// recompute the binding without holding the deployment salt.
#[must_use]
pub fn denial_subject_hash(salt_fingerprint: &str, subject_id: &str) -> String {
    let digest = clio_store::sha256_hex(salt_fingerprint.as_bytes(), subject_id);
    digest[..32].to_owned()
}

/// The durable proof a peer answers a denial operation with.
///
/// A bare transport receipt is not a proof: only the answering deployment's
/// recorded generation, the subject identity it binds, its own device id, and
/// the fingerprint of the salt it hashed under make an acknowledgement
/// authoritative, and a restore into another deployment cannot reproduce one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DenialProof {
    /// The durable request reference the denial was recorded under.
    pub request_ref: String,
    /// Device id of the deployment that recorded the denial.
    pub peer_id: String,
    /// The recorded subject's identity under that deployment's salt.
    pub subject_hash: String,
    /// The authority generation the denial was recorded at.
    pub generation: u64,
    /// The answering deployment's salt fingerprint.
    pub salt_fingerprint: String,
}

/// What one proof must bind before it counts as an acknowledgement.
pub struct DenialProofBinding<'a> {
    /// The action the proof answers for.
    pub action: &'a str,
    /// The durable request reference the proof must carry.
    pub request_ref: &'a str,
    /// The peer that must have answered.
    pub peer_id: &'a str,
    /// The subject the denial covers.
    pub subject_id: &'a str,
    /// The salt fingerprint of the deployment that must have answered.
    pub answering_fingerprint: &'a str,
}

impl<'a> DenialProofBinding<'a> {
    /// The binding one acknowledgement for `m` must carry: the denial action,
    /// the carrier's request reference, the answering peer, the subject in
    /// the mutation's entity id, and the fingerprint of the deployment that
    /// must have answered.
    #[must_use]
    pub fn for_carrier(
        m: &'a SyncMutation,
        carrier: &'a DenialCarrier,
        peer_id: &'a str,
        answering_fingerprint: &'a str,
    ) -> Self {
        Self {
            action: DENIAL_ACTION,
            request_ref: &carrier.request_ref,
            peer_id,
            subject_id: &m.entity_id,
            answering_fingerprint,
        }
    }
}

impl DenialProof {
    /// Verify this proof binds the expected action, request, peer, subject,
    /// and answering deployment. A proof that binds anything else is refused
    /// rather than acknowledged.
    ///
    /// # Errors
    /// A blank binding field, an action other than `denial`, a generation of
    /// zero, a different request reference, peer, or fingerprint, or a
    /// subject digest that does not recompute from the subject under the
    /// proof's own fingerprint.
    pub fn verify(&self, binding: &DenialProofBinding<'_>) -> Result<(), AmError> {
        if binding.action != DENIAL_ACTION {
            return Err(refusal(format!(
                "a denial proof answers for `{DENIAL_ACTION}`, not `{}`",
                binding.action
            )));
        }
        for (field, value) in [
            ("request reference", binding.request_ref),
            ("peer id", binding.peer_id),
            ("subject id", binding.subject_id),
            ("salt fingerprint", binding.answering_fingerprint),
        ] {
            if value.trim().is_empty() {
                return Err(refusal(format!(
                    "a denial acknowledgement requires a {field}"
                )));
            }
        }
        if self.generation == 0 {
            return Err(refusal("a denial proof requires a recorded generation"));
        }
        if self.request_ref != binding.request_ref {
            return Err(refusal(
                "the denial proof binds a different request reference",
            ));
        }
        if self.peer_id != binding.peer_id {
            return Err(refusal("the denial proof was answered by a different peer"));
        }
        if self.salt_fingerprint != binding.answering_fingerprint {
            return Err(refusal(
                "the denial proof was answered under a different deployment salt",
            ));
        }
        if self.subject_hash != denial_subject_hash(&self.salt_fingerprint, binding.subject_id) {
            return Err(refusal("the denial proof binds a different subject"));
        }
        Ok(())
    }
}

/// One durable proof riding a push acknowledgement for one denial mutation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DenialAck {
    /// The mutation id the acknowledged denial operation carried.
    pub sync_mutation_id: String,
    /// The durable proof the answering deployment's authority returned.
    pub proof: DenialProof,
}

/// The verdict for one served bank: `Denied` when a recorded denial covers
/// it, `AuthorityUnavailable` when the authority cannot be read, else `Clear`.
#[must_use]
pub fn bank_verdict(denial: &dyn ErasureDenial, bank: &str) -> DenialVerdict {
    denial.verdict(&denied_bank_marker(bank))
}

/// The refusal for an acknowledgement that does not bind.
fn refusal(message: impl Into<String>) -> AmError {
    AmError::new(ErrorCode::Forbidden, message.into())
}

#[cfg(test)]
#[path = "denial_tests.rs"]
mod denial_tests;
