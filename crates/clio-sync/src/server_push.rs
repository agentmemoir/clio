// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the sync server's push route: request
//! decoding, per-mutation and per-covered-bank bank authorization, the
//! per-bank apply lock, the guarded apply of every mutation, the dead-letter
//! quarantine of refused mutations, and the acknowledgement that carries each
//! denial operation's durable proof.
//!
//! ## Owns
//! - `handle_push` and the push acknowledgement body it writes
//! - The authorization refusals (the envelope bank, each mutation, and every
//!   covered bank of a denial carrier) and their quarantine placement
//! - The denial acknowledgement list a push returns
//!
//! ## Does not own
//! - The pull and status routes, the server options, and construction
//!   (`server`)
//! - Apply semantics, dedupe, and dead-letter policy (`apply` / `denial_guard`)
//! - Credential resolution and scoped bank lists (`credential`, `server`)
//! - Connection lifecycle and response writers (`server_conn`)
//!
//! ## Boundary
//!
//! Changes to how a push body becomes apply calls and an acknowledgement
//! should normally require changes here. Changes to apply or transport
//! semantics should not.
//!
//! Keep this module focused on the push route.

use crate::apply::{
    ApplyEngine, ApplyOutcome, apply_mutation_guarded, apply_mutation_guarded_relaying, quarantine,
};
use crate::credential::AuthOutcome;
use crate::denial::DenialAck;
use crate::protocol::{PushAck, SyncContent, SyncMutation, encode_cursor};
use crate::server::{ServeOptions, bank_authorized};
use crate::server_conn::{write_json, write_status_error};
use clio_store::DenialVerdict;
use clio_types::{AmError, ErrorCode};
use serde_json::Value;
use std::net::TcpStream;

/// The refusal a served mutation carrying plaintext content gets. It names
/// what this node stores and the remedy the sending peer can act on: seal the
/// row with a sync key and push it again.
const PLAINTEXT_CONTENT_REFUSAL: &str = "refused plaintext content: this node stores memory only as client-sealed content, and a \
     plaintext row carries no authenticated envelope, so it cannot be told apart from one a \
     relay fabricated or downgraded from sealed content. Reseal the row on the sending node \
     with a sync key and push again";

/// The refusal a sealed mutation for a subject a recorded denial covers gets.
/// The subject was resolved here, under this peer's own sync key, so the row is
/// quarantined as the erased subject rather than as an unreadable envelope.
const SEALED_ERASED_SUBJECT_REFUSAL: &str = "refused sealed content for a subject a recorded erasure denial covers: the \
     person's data was erased, so the row is quarantined instead of applied";

/// What the sealed subject check allows for one mutation.
enum SealedCheck {
    /// The row may take the ordinary apply path.
    Apply,
    /// The row was quarantined as the erased subject.
    Refused,
}

/// Refuse a sealed mutation whose resolved subject a recorded denial covers.
///
/// A sealed row keeps its subject inside the ciphertext, so the apply guard's
/// own read sees no subject at all. The serving peer holds the sync key, so the
/// subject is resolved here, before the apply path writes anything: a row for a
/// denied subject is quarantined as the erased subject, and a row this peer
/// cannot open keeps the behavior it had before this check.
///
/// # Errors
/// The quarantine write, and the authority's own refusal when it cannot be read.
fn check_sealed_subject(
    engine: &ApplyEngine<'_>,
    options: &ServeOptions,
    bank: &str,
    m: &SyncMutation,
    envelope: &str,
    rejected: &mut Vec<String>,
) -> Result<SealedCheck, AmError> {
    let (Some(denial), Some(key)) = (options.denial.as_deref(), options.sync_key.as_deref()) else {
        return Ok(SealedCheck::Apply);
    };
    match crate::denial_guard::sealed_verdict(denial, key, m) {
        DenialVerdict::Clear => Ok(SealedCheck::Apply),
        DenialVerdict::Denied => {
            refuse(
                engine,
                bank,
                m,
                envelope,
                crate::protocol::REASON_ERASED_SUBJECT,
                SEALED_ERASED_SUBJECT_REFUSAL,
                rejected,
            )?;
            Ok(SealedCheck::Refused)
        }
        DenialVerdict::AuthorityUnavailable => Err(AmError::new(
            ErrorCode::ConfigCorrupt,
            "the local erasure-denial authority is unreadable; refusing the mutation",
        )),
    }
}

/// Quarantine one refused mutation under the envelope's authorized bank and
/// record its id as rejected, so a refusal never writes into another bank and
/// never leaves the peer guessing.
fn refuse(
    engine: &ApplyEngine<'_>,
    bank: &str,
    m: &SyncMutation,
    envelope: &str,
    reason_code: &str,
    detail: &str,
    rejected: &mut Vec<String>,
) -> Result<(), AmError> {
    quarantine(engine, bank, m, envelope, reason_code, detail)?;
    rejected.push(m.sync_mutation_id.clone());
    Ok(())
}

/// Apply one authorized mutation and record its outcome: the proof a denial
/// operation answers with, and whether the peer is told accepted or rejected.
///
/// A row sealed under this peer's key is opened here, so the subject guard and
/// the apply effects see the payload the writer sealed, while the journal keeps
/// relaying that same sealed row to every reader downstream.
fn apply_and_record(
    engine: &ApplyEngine<'_>,
    options: &ServeOptions,
    m: &SyncMutation,
    envelope: &str,
    accepted: &mut Vec<String>,
    rejected: &mut Vec<String>,
    denial_acks: &mut Vec<DenialAck>,
) -> Result<(), AmError> {
    let denial = options.denial.as_deref();
    let result = match options
        .sync_key
        .as_deref()
        .and_then(|key| crate::denial_guard::sealed_content(key, m))
    {
        Some(content) => {
            let opened = SyncMutation {
                content: SyncContent::Plain(content),
                ..m.clone()
            };
            apply_mutation_guarded_relaying(engine, denial, &opened, envelope, m)?
        }
        None => apply_mutation_guarded(engine, denial, m, envelope)?,
    };
    // The durable record is written before this acknowledgement: the proof is
    // the authority's own record of it, never a transport receipt, and a
    // redelivery returns the same proof.
    if let Some(proof) = result.denial_proof {
        denial_acks.push(DenialAck {
            sync_mutation_id: m.sync_mutation_id.clone(),
            proof,
        });
    }
    match result.outcome {
        ApplyOutcome::DeadLettered { .. } => rejected.push(m.sync_mutation_id.clone()),
        _ => accepted.push(m.sync_mutation_id.clone()),
    }
    Ok(())
}

/// The bank and the mutations one push body carries.
///
/// # Errors
/// A body that is not JSON, or a mutation that does not decode as one.
fn decode_push(body: &str) -> Result<(String, Vec<SyncMutation>), AmError> {
    let payload: Value = serde_json::from_str(body)
        .map_err(|e| AmError::new(ErrorCode::InvalidArgument, format!("bad push body: {e}")))?;
    let bank = payload
        .get("bank")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let mutations = payload
        .get("mutations")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .map(|m| serde_json::from_value(m.clone()))
                .collect::<Result<Vec<SyncMutation>, _>>()
        })
        .transpose()
        .map_err(|e| AmError::new(ErrorCode::InvalidArgument, format!("bad mutation: {e}")))?
        .unwrap_or_default();
    Ok((bank, mutations))
}

pub(crate) fn handle_push(
    options: &ServeOptions,
    stream: TcpStream,
    body: &str,
    auth: &AuthOutcome,
) -> Result<(), AmError> {
    let (bank, mutations) = decode_push(body)?;
    if !bank_authorized(options, auth, &bank) {
        return write_status_error(
            stream,
            403,
            &AmError::new(ErrorCode::Forbidden, "bank not authorized"),
        );
    }
    // The apply path below is check-then-act over the item row, the apply
    // guard, and the journal, and this server runs one worker per accepted
    // connection. Taking the bank's lock before the first apply makes the
    // dedupe checks and the LWW resolution mean what they say, whatever order
    // two pushes for the same bank arrive in; banks never wait on each other.
    //
    // A poisoned lock means a previous apply panicked mid-push; the apply
    // steps are transactional, so the next push proceeds.
    let bank_lock = options.bank_locks.for_bank(&bank);
    let _bank_guard = bank_lock
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let engine = ApplyEngine {
        store: options.store.as_ref(),
        sync: options.sync.as_ref(),
        device_id: options.device_id.clone(),
        kms: Some(options.kms.as_ref()),
    };
    let mut accepted = Vec::new();
    let mut rejected = Vec::new();
    let mut denial_acks: Vec<DenialAck> = Vec::new();
    for m in mutations {
        let envelope = serde_json::to_string(&m).expect("serialize mutation");
        // The envelope bank authorized above, but each mutation must also be
        // authorized: a mutation for a bank this credential cannot reach is
        // quarantined under the envelope's authorized bank — a refusal never
        // writes into another bank — and the ack reports it rejected.
        if !bank_authorized(options, auth, &m.bank) {
            refuse(
                &engine,
                &bank,
                &m,
                &envelope,
                crate::protocol::REASON_UNAUTHORIZED_BANK,
                &format!("mutation targets unauthorized bank {}", m.bank),
                &mut rejected,
            )?;
            continue;
        }
        // A denial carrier records one serve marker per covered bank before
        // the proof exists, so every covered bank must be authorized first:
        // a credential for one bank must never durably refuse another. A
        // carrier that does not decode is not a carrier, and the plaintext
        // refusal below quarantines it rather than reading it as one.
        if let Ok(Some(carrier)) = crate::denial::denial_carrier(&m)
            && let Some(unauthorized) = carrier
                .covered_banks()
                .iter()
                .find(|covered| !bank_authorized(options, auth, covered))
        {
            refuse(
                &engine,
                &bank,
                &m,
                &envelope,
                crate::protocol::REASON_UNAUTHORIZED_BANK,
                &format!("denial carrier covers unauthorized bank {unauthorized}"),
                &mut rejected,
            )?;
            continue;
        }
        // Content crosses this route only as client-sealed bytes. A plaintext
        // row carries no authenticated envelope, so whoever holds the bank
        // token — or a relay in front of the peer — could have invented it or
        // downgraded a sealed row to it, and applying it here would put
        // unauthenticated text in this node's memory. The denial carrier is the
        // one plaintext shape the route reads: it is content-free, and the
        // authority resolves its banks as markers rather than as content.
        if matches!(m.content, SyncContent::Plain(_))
            && !matches!(crate::denial::denial_carrier(&m), Ok(Some(_)))
        {
            refuse(
                &engine,
                &bank,
                &m,
                &envelope,
                crate::protocol::REASON_MALFORMED,
                PLAINTEXT_CONTENT_REFUSAL,
                &mut rejected,
            )?;
            continue;
        }
        // A sealed row's subject can only be resolved with this peer's own key.
        match check_sealed_subject(&engine, options, &bank, &m, &envelope, &mut rejected) {
            Ok(SealedCheck::Apply) => {}
            Ok(SealedCheck::Refused) => continue,
            Err(err) => return write_status_error(stream, 500, &err),
        }
        if let Err(err) = apply_and_record(
            &engine,
            options,
            &m,
            &envelope,
            &mut accepted,
            &mut rejected,
            &mut denial_acks,
        ) {
            // A transient refusal: no ack, the client retries unchanged, so the
            // authority's own code is kept and a damaged authority stays
            // distinguishable from a storage failure.
            return write_status_error(stream, 500, &err);
        }
    }
    let cursor = encode_cursor(&bank, options.sync.journal_max_seq(&bank)?);
    write_json(
        stream,
        200,
        &serde_json::to_value(PushAck {
            cursor,
            accepted,
            rejected,
            denial_acks,
        })
        .expect("serialize push ack"),
    )
}

#[cfg(test)]
#[path = "server_push_denial_tests.rs"]
mod server_push_denial_tests;
