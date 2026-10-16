// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for delivering one denial action to one sync
//! peer over the existing client push route: it builds the operation's one
//! idempotent mutation, pushes it, and classifies the peer's answer as a
//! verified durable proof, a refusal, or an unreachable peer.
//!
//! ## Owns
//! - `deliver_denial`, `DenialTarget`, and `DeliveryOutcome`
//!
//! ## Does not own
//! - The denial wire vocabulary: the carrier, the idempotency id, and the
//!   proof (`denial` owns those)
//! - The server's denial routes and guards (`server`, `server_push`)
//! - Recording the caller's per-peer action or its acknowledgement
//!   (`clio-store` `peer_actions`; the compliance helpers call this module)
//!
//! ## Boundary
//!
//! Changes to how one denial operation is delivered, or to what counts as a
//! delivered proof, should normally require changes here. Changes to what a
//! denial carrier or proof is should not.
//!
//! Keep this module focused on one denial delivery to one peer.
//!
//! # Duplicate delivery
//!
//! The mutation id is derived from the request reference alone, so a retry
//! resumes the same id: the peer's redelivery path re-proves the durable
//! record it already holds without repeating any effect, and this module
//! returns that same proof.

use crate::client::SyncClientConfig;
use crate::denial::{
    DenialCarrier, DenialProof, DenialProofBinding, denial_mutation, denial_mutation_id,
};
use crate::http::post_json_status;
use crate::protocol::{PushAck, SyncMutation, SyncPayload};
use clio_types::{AmError, ErrorCode};

/// What one denial delivery is verified against: the expected request, peer,
/// bank, subject, covered banks, and answering deployment.
pub struct DenialTarget<'a> {
    /// The durable request reference the denial operates under.
    pub request_ref: &'a str,
    /// The peer that must answer: its device id.
    pub peer_id: &'a str,
    /// The bank the push rides; the carrier is pushed under this bank.
    pub bank: &'a str,
    /// The subject the denial covers (the mutation's entity id).
    pub subject_id: &'a str,
    /// The banks the carrier covers. The peer records one serve guard for each.
    pub covering: &'a [&'a str],
    /// The salt fingerprint the answering deployment must prove.
    pub salt_fingerprint: &'a str,
}

/// What one denial delivery attempt did.
#[derive(Debug, Clone, PartialEq)]
pub enum DeliveryOutcome {
    /// The peer answered with a durable proof that verified against the
    /// expected binding. Only this outcome may record the action delivered.
    Delivered {
        /// The verified durable proof the peer answered with.
        proof: DenialProof,
    },
    /// The peer refused the operation, or answered without a proof that
    /// verifies against the expected binding. A transport 200 alone is never
    /// an acknowledgement.
    Refused {
        /// Why the peer's answer does not acknowledge the operation.
        reason: String,
    },
    /// No durable effect at the peer: a transport failure or a retryable
    /// answer. The caller keeps the action pending; a retry resumes the same
    /// mutation id.
    Unreachable {
        /// Why no durable answer was obtained.
        reason: String,
    },
}

/// Deliver one denial operation to the peer `cfg` names.
///
/// # Errors
/// A blank target field, a target covering no bank, a mutation the carrier
/// refuses, or no configured remote: the caller's own input, not a delivery
/// outcome.
pub fn deliver_denial(
    cfg: &SyncClientConfig,
    target: &DenialTarget<'_>,
) -> Result<DeliveryOutcome, AmError> {
    let remote = cfg.remote.clone().ok_or_else(|| {
        AmError::new(
            ErrorCode::InvalidArgument,
            "a denial delivery requires the peer's remote endpoint",
        )
    })?;
    for (field, value) in [
        ("request reference", target.request_ref),
        ("peer id", target.peer_id),
        ("bank", target.bank),
        ("subject id", target.subject_id),
        ("salt fingerprint", target.salt_fingerprint),
    ] {
        if value.trim().is_empty() {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                format!("a denial delivery requires a {field}"),
            ));
        }
    }
    let carrier = DenialCarrier {
        request_ref: target.request_ref.to_owned(),
        banks: target
            .covering
            .iter()
            .map(|bank| (*bank).to_owned())
            .collect(),
    };
    carrier.validate()?;
    let mutation = denial_mutation(
        target.bank,
        target.subject_id,
        &carrier,
        &cfg.device_id,
        &crate::clock::now_utc(),
    );
    let payload = SyncPayload {
        device_id: cfg.device_id.clone(),
        bank: target.bank.to_owned(),
        mutations: vec![mutation.clone()],
    };
    let body = serde_json::to_value(&payload).expect("serialize the push payload");
    // The shared exchange keeps the peer's status, so a peer's refusal (4xx)
    // stays distinct from a retryable answer (5xx) and from a transport
    // failure, exactly as the client's pull path reads its own answers.
    match post_json_status(&remote, "/sync/push", cfg.token.as_deref(), &body) {
        Ok((200, value)) => Ok(match serde_json::from_value::<PushAck>(value) {
            Ok(ack) => acknowledge(&mutation, &carrier, target, &ack),
            Err(err) => unreachable_answer(format!(
                "the peer's push acknowledgment is unreadable: {err}"
            )),
        }),
        Ok((status, value)) if (400..500).contains(&status) => Ok(DeliveryOutcome::Refused {
            reason: format!("the peer refused the denial operation (status {status}): {value}"),
        }),
        Ok((status, value)) => Ok(unreachable_answer(format!(
            "the peer did not durably accept the denial operation (status {status}): {value}"
        ))),
        Err(err) => Ok(unreachable_answer(format!(
            "the peer is unreachable: {}",
            err.message
        ))),
    }
}

/// Classify one readable push acknowledgement: the proof for this operation
/// must be present and must bind the expected target.
fn acknowledge(
    mutation: &SyncMutation,
    carrier: &DenialCarrier,
    target: &DenialTarget<'_>,
    ack: &PushAck,
) -> DeliveryOutcome {
    let expected = denial_mutation_id(target.request_ref);
    let Some(entry) = ack
        .denial_acks
        .iter()
        .find(|entry| entry.sync_mutation_id == expected)
    else {
        let reason = if ack.rejected.contains(&expected) {
            "the peer rejected the denial operation".to_owned()
        } else {
            "the peer answered without a durable denial proof".to_owned()
        };
        return DeliveryOutcome::Refused { reason };
    };
    let binding =
        DenialProofBinding::for_carrier(mutation, carrier, target.peer_id, target.salt_fingerprint);
    match entry.proof.verify(&binding) {
        Ok(()) => DeliveryOutcome::Delivered {
            proof: entry.proof.clone(),
        },
        Err(err) => DeliveryOutcome::Refused {
            reason: format!(
                "the peer's proof does not bind this operation: {}",
                err.message
            ),
        },
    }
}

/// An answer with no durable effect at the peer: a retry resumes the same
/// mutation id.
fn unreachable_answer(reason: String) -> DeliveryOutcome {
    DeliveryOutcome::Unreachable { reason }
}
