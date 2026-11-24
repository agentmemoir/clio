// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the denial guards the sync routes apply: the
//! subject guard before any subject data is written, the durable record and
//! proof one denial carrier produces, the bank guard before a pull is served,
//! and the reserved bank markers that make the serve answer durable.
//!
//! ## Owns
//! - `content_subject_id` / `subject_verdict`: which payload shapes are
//!   subject-keyed and the three-way verdict for one payload's subject
//! - `sealed_content` / `sealed_subject_id` / `sealed_verdict`: the payload a
//!   serving peer's sync key opens, the subject inside it, and the same
//!   three-way verdict for that subject
//! - `guard_subject`: the verdict a payload's subject must pass before an
//!   upsert or mint path may run
//! - `record_carrier_proof` / `apply_carrier` / `redelivery_proof`: recording
//!   one denial operation durably and returning the proof that acknowledges it
//! - `pull_refusal`: the refusal for a bank a recorded denial covers
//!
//! ## Does not own
//! - The denial operation's wire vocabulary (`denial` owns the carrier, the
//!   proof, and the marker namespace)
//! - The authority and its journal (`ErasureDenial`, implemented outside this
//!   crate)
//! - Route statuses and the acknowledgement body (`server` / `server_push`)
//! - Apply transitions and dead-letter policy (`apply`)
//!
//! ## Boundary
//!
//! Changes to when a denial refuses an apply or a pull should normally require
//! changes here. Changes to the wire carrier or the proof should not.
//!
//! Keep this module focused on the denial guards over the sync paths.

use crate::apply::{ApplyEngine, ApplyOutcome, ApplyResult, dead_letter, quarantine};
use crate::crypto::SyncCipherKey;
use crate::denial::{
    DenialCarrier, DenialProof, DenialProofBinding, bank_verdict, denial_carrier,
    denial_subject_hash, denied_bank_marker,
};
use crate::protocol::{
    ENTITY_KIND_BELIEF, ENTITY_KIND_FAILURE, ENTITY_KIND_ITEM, ENTITY_KIND_TASK,
    REASON_UNAUTHORIZED_ACTOR, REASON_UNAUTHORIZED_BANK, SyncContent, SyncMutation,
};
use clio_store::{DenialVerdict, ErasureDenial};
use clio_types::{AmError, ErrorCode};
use serde_json::Value;

/// The refusal a denial carrier from a credential whose actor carries no
/// erasure privilege gets.
const DENIAL_ACTOR_REFUSAL: &str = "refused denial carrier: this credential's actor may not record a denial. An \
     erasure denial refuses a subject for every bank on this node and cannot be undone, so it requires a harness \
     or system actor";

/// What the presenting credential may do with a denial carrier.
///
/// A carrier records one serve marker per covered bank before its proof exists,
/// so every covered bank must be authorized; and it durably refuses its subject
/// on this whole node, so its actor must carry the erasure privilege. A call
/// site passes `None` only when the mutation arrived without a credential to
/// check.
pub(crate) trait CarrierAuthority {
    /// Whether the presenting token's actor may durably record a denial.
    fn may_record_denial(&self) -> bool;

    /// Whether the presenting credential may reach `bank` on this node.
    fn bank_authorized(&self, bank: &str) -> bool;
}

/// The refusal the presenting credential earns for `carrier`, if any: the
/// reason code and detail for an actor without erasure privilege, or for a
/// covered bank the credential cannot reach.
fn carrier_refusal(
    authority: Option<&dyn CarrierAuthority>,
    carrier: &DenialCarrier,
) -> Option<(&'static str, String)> {
    let authority = authority?;
    if !authority.may_record_denial() {
        return Some((REASON_UNAUTHORIZED_ACTOR, DENIAL_ACTOR_REFUSAL.to_owned()));
    }
    carrier
        .covered_banks()
        .into_iter()
        .find(|bank| !authority.bank_authorized(bank))
        .map(|bank| {
            (
                REASON_UNAUTHORIZED_BANK,
                format!("denial carrier covers unauthorized bank {bank}"),
            )
        })
}

/// The subject id one payload carries, for the record kinds that are
/// subject-keyed: an item or belief record carries it at the top level, a task
/// or failure carries it on its carrier item. `None` for a kind that carries
/// no subject id at all — associations, triples, personas, denial carriers —
/// and for an opaque client-encrypted payload, which carries nothing readable.
#[must_use]
pub fn content_subject_id<'a>(entity_kind: &str, content: &'a Value) -> Option<&'a str> {
    let value = match entity_kind {
        ENTITY_KIND_ITEM | ENTITY_KIND_BELIEF => content.get("subject_id"),
        ENTITY_KIND_TASK | ENTITY_KIND_FAILURE => {
            content.get("item").and_then(|item| item.get("subject_id"))
        }
        _ => None,
    };
    value
        .and_then(Value::as_str)
        .filter(|subject_id| !subject_id.is_empty())
}

/// The verdict for the subject a mutation carries: `Clear` when the payload
/// carries no readable subject id.
#[must_use]
pub fn subject_verdict(
    denial: &dyn ErasureDenial,
    entity_kind: &str,
    content: &Value,
) -> DenialVerdict {
    match content_subject_id(entity_kind, content) {
        Some(subject_id) => denial.verdict(subject_id),
        None => DenialVerdict::Clear,
    }
}

/// The content one sealed payload carries, opened under the serving peer's sync
/// key, or `None` when this peer cannot read it.
///
/// A sealed row keeps everything inside the ciphertext, so the plaintext readers
/// below and the apply path see no content at all. `None` is the answer in every
/// case this peer cannot speak to: content that is not sealed, a ciphertext the
/// key does not open, and a payload that is not JSON. A row this peer cannot
/// read therefore keeps the behavior it had before the peer held a key.
#[must_use]
pub fn sealed_content(key: &SyncCipherKey, m: &SyncMutation) -> Option<Value> {
    let SyncContent::Cipher(cipher) = &m.content else {
        return None;
    };
    let bytes = key.open(cipher, &m.execution_view().aad()).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// The subject id one sealed payload carries, once the serving peer's sync key
/// opens it, or `None` when there is nothing to resolve here.
///
/// The companion case is the same as [`sealed_content`]'s: an unsealed row, one
/// this key does not open, and a payload or kind that carries no subject id all
/// answer `None`, so a row this peer cannot read is never quarantined on a guess.
#[must_use]
pub fn sealed_subject_id(key: &SyncCipherKey, m: &SyncMutation) -> Option<String> {
    content_subject_id(&m.entity_kind, &sealed_content(key, m)?).map(str::to_owned)
}

/// The verdict for the subject a sealed mutation carries: `Clear` when the row
/// is unsealed, cannot be opened, or carries no readable subject id.
#[must_use]
pub fn sealed_verdict(
    denial: &dyn ErasureDenial,
    key: &SyncCipherKey,
    m: &SyncMutation,
) -> DenialVerdict {
    match sealed_subject_id(key, m) {
        Some(subject_id) => denial.verdict(&subject_id),
        None => DenialVerdict::Clear,
    }
}

/// Guard the subject data one mutation carries against the current denial.
///
/// `Ok(None)` when the mutation may proceed; `Ok(Some(outcome))` when a
/// recorded denial quarantines it, so a stale create can never become
/// readable; and an error when the authority cannot be read — a refusal that
/// leaves nothing behind, because an unreadable authority is not evidence of
/// a clear subject.
///
/// # Errors
/// The authority's own error when it is unavailable.
pub(crate) fn guard_subject(
    engine: &ApplyEngine<'_>,
    denial: &dyn ErasureDenial,
    m: &SyncMutation,
    envelope_json: &str,
) -> Result<Option<ApplyOutcome>, AmError> {
    match subject_verdict(denial, &m.entity_kind, m.content_value()) {
        DenialVerdict::Clear => Ok(None),
        DenialVerdict::Denied => dead_letter(
            engine,
            m,
            envelope_json,
            &AmError::new(
                ErrorCode::ErasedSubject,
                "subject is covered by a recorded erasure denial",
            ),
        )
        .map(Some),
        DenialVerdict::AuthorityUnavailable => Err(AmError::new(
            ErrorCode::ConfigCorrupt,
            "the local erasure-denial authority is unreadable; refusing the mutation",
        )),
    }
}

/// Durably record the denial one carrier names and return the proof that
/// acknowledges it.
///
/// Every covered bank is recorded first, under its own reserved marker, so the
/// serve-side answer for each bank is durable before any proof exists; the
/// subject denial follows and its generation is the one the proof carries. A
/// redelivery repeats no effect: an already-recorded identity makes no change
/// and returns the generation the authority carries.
///
/// # Errors
/// The authority's own error when it refuses or cannot be read, and a refusal
/// when the authority reports no salt fingerprint.
pub(crate) fn record_carrier_proof(
    denial: &dyn ErasureDenial,
    peer_id: &str,
    m: &SyncMutation,
    carrier: &DenialCarrier,
) -> Result<DenialProof, AmError> {
    let fingerprint = denial.salt_fingerprint();
    if fingerprint.trim().is_empty() {
        return Err(AmError::new(
            ErrorCode::ConfigCorrupt,
            "the denial authority reports no salt fingerprint; refusing the acknowledgement",
        ));
    }
    for bank in carrier.covered_banks() {
        denial.record_denial(&denied_bank_marker(&bank), &[bank])?;
    }
    let generation = denial.record_denial(&m.entity_id, &carrier.banks)?;
    let proof = DenialProof {
        request_ref: carrier.request_ref.clone(),
        peer_id: peer_id.to_owned(),
        subject_hash: denial_subject_hash(&fingerprint, &m.entity_id),
        generation,
        salt_fingerprint: fingerprint,
    };
    // The proof is checked before it can travel: a proof that does not bind
    // the operation is never written into an acknowledgement.
    proof.verify(&DenialProofBinding::for_carrier(
        m,
        carrier,
        peer_id,
        &proof.salt_fingerprint,
    ))?;
    Ok(proof)
}

/// Record the denial one mutation carries, journal it, and return the outcome
/// with its durable proof.
///
/// `Ok(None)` when the mutation is not a denial operation, so the caller keeps
/// its ordinary apply behavior. The presenting credential answers here before
/// any marker, denial, or journal row is written: a token whose actor carries
/// no erasure privilege, or that cannot reach every covered bank, is refused
/// and quarantined. A sealed carrier is opened by the push route and reaches
/// this gate as plaintext, so the same check covers both carriers. A malformed
/// carrier is a permanent reject; an authority failure propagates, so no
/// acknowledgement is written.
///
/// # Errors
/// Authority or storage failures (retryable).
pub(crate) fn apply_carrier(
    engine: &ApplyEngine<'_>,
    denial: &dyn ErasureDenial,
    authority: Option<&dyn CarrierAuthority>,
    m: &SyncMutation,
    envelope_json: &str,
) -> Result<Option<ApplyResult>, AmError> {
    let carrier = match denial_carrier(m) {
        Ok(Some(carrier)) => carrier,
        Ok(None) => return Ok(None),
        Err(err) => {
            return dead_letter(engine, m, envelope_json, &err)
                .map(|outcome| Some(ApplyResult::plain(outcome)));
        }
    };
    if let Some((reason_code, detail)) = carrier_refusal(authority, &carrier) {
        return quarantine(engine, &m.bank, m, envelope_json, reason_code, &detail)
            .map(|outcome| Some(ApplyResult::plain(outcome)));
    }
    let proof = record_carrier_proof(denial, &engine.device_id, m, &carrier)?;
    // The journal row is what relays the denial onward, so the record that was
    // just written stays reachable by pull. A concurrent redelivery that
    // journaled first leaves this append uninserted and still receives the
    // same proof: the authority write above repeated no effect.
    let _ = engine
        .sync
        .journal_append(&crate::journal_row::journal_row(engine, m)?)?;
    Ok(Some(ApplyResult::proven(ApplyOutcome::Applied, proof)))
}

/// The durable proof for a denial mutation whose journal row already exists.
///
/// `Ok(None)` when the mutation is not a denial operation. A malformed or
/// unreadable carrier refuses rather than acknowledging: nothing may be
/// acknowledged from a redelivery the authority cannot bind.
///
/// # Errors
/// The authority's own error, or a malformed carrier refusal.
pub(crate) fn redelivery_proof(
    denial: &dyn ErasureDenial,
    peer_id: &str,
    m: &SyncMutation,
) -> Result<Option<DenialProof>, AmError> {
    match denial_carrier(m)? {
        Some(carrier) => record_carrier_proof(denial, peer_id, m, &carrier).map(Some),
        None => Ok(None),
    }
}

/// The refusal a redelivery of an already-journaled denial carrier earns when
/// the presenting credential may not record it.
///
/// A journaled id answers a redelivery with a fresh proof, and that proof
/// re-records every covered bank's marker. The credential gate therefore runs
/// before the proof is rebuilt. `Ok(None)` means the carrier may re-prove.
///
/// # Errors
/// The quarantine write.
pub(crate) fn guard_redelivery(
    engine: &ApplyEngine<'_>,
    authority: Option<&dyn CarrierAuthority>,
    m: &SyncMutation,
    envelope_json: &str,
) -> Result<Option<ApplyOutcome>, AmError> {
    let Ok(Some(carrier)) = denial_carrier(m) else {
        return Ok(None);
    };
    let Some((reason_code, detail)) = carrier_refusal(authority, &carrier) else {
        return Ok(None);
    };
    quarantine(engine, &m.bank, m, envelope_json, reason_code, &detail).map(Some)
}

/// The refusal for a pull whose bank a recorded denial covers, or whose
/// authority cannot be read; `Ok(())` when the bank may be served.
///
/// The refusal is per bank because a journal row carries no readable subject,
/// so a served row cannot be resolved to the subject a denial names. Refusing
/// the whole pull is the only fail-closed answer for a covered or unreadable
/// bank; an unrelated bank is unaffected.
///
/// # Errors
/// [`ErrorCode::ErasedSubject`] for a covered bank, [`ErrorCode::ConfigCorrupt`]
/// for an authority that cannot be read.
pub(crate) fn pull_refusal(denial: Option<&dyn ErasureDenial>, bank: &str) -> Result<(), AmError> {
    let Some(denial) = denial else {
        return Ok(());
    };
    match bank_verdict(denial, bank) {
        DenialVerdict::Clear => Ok(()),
        DenialVerdict::Denied => Err(AmError::new(
            ErrorCode::ErasedSubject,
            format!("bank `{bank}` is covered by a recorded erasure denial; refusing the pull"),
        )),
        DenialVerdict::AuthorityUnavailable => Err(AmError::new(
            ErrorCode::ConfigCorrupt,
            "the local erasure-denial authority is unreadable; refusing the pull",
        )),
    }
}

#[cfg(test)]
#[path = "denial_guard_sealed_tests.rs"]
mod denial_guard_sealed_tests;
