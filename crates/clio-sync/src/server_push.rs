// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the sync server's push route: request
//! decoding, per-mutation bank authorization, the per-bank apply lock, the
//! guarded apply of every mutation, the dead-letter quarantine of refused
//! mutations, and the acknowledgement that carries each denial operation's
//! durable proof.
//!
//! ## Owns
//! - `handle_push` and the push acknowledgement body it writes
//! - The per-mutation authorization refusal and its quarantine placement
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

use crate::apply::{ApplyEngine, ApplyOutcome, apply_mutation_guarded, quarantine};
use crate::credential::AuthOutcome;
use crate::denial::DenialAck;
use crate::protocol::{PushAck, SyncMutation, encode_cursor};
use crate::server::{ServeOptions, bank_authorized};
use crate::server_conn::{write_json, write_status_error};
use clio_types::{AmError, ErrorCode};
use serde_json::Value;
use std::net::TcpStream;

pub(crate) fn handle_push(
    options: &ServeOptions,
    stream: TcpStream,
    body: &str,
    auth: &AuthOutcome,
) -> Result<(), AmError> {
    let payload: Value = serde_json::from_str(body)
        .map_err(|e| AmError::new(ErrorCode::InvalidArgument, format!("bad push body: {e}")))?;
    let bank = payload
        .get("bank")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    if !bank_authorized(options, auth, &bank) {
        return write_status_error(
            stream,
            403,
            &AmError::new(ErrorCode::Forbidden, "bank not authorized"),
        );
    }
    let mutations: Vec<SyncMutation> = payload
        .get("mutations")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .map(|m| serde_json::from_value(m.clone()))
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()
        .map_err(|e| AmError::new(ErrorCode::InvalidArgument, format!("bad mutation: {e}")))?
        .unwrap_or_default();
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
            quarantine(
                &engine,
                &bank,
                &m,
                &envelope,
                crate::protocol::REASON_UNAUTHORIZED_BANK,
                &format!("mutation targets unauthorized bank {}", m.bank),
            )?;
            rejected.push(m.sync_mutation_id);
            continue;
        }
        match apply_mutation_guarded(&engine, options.denial.as_deref(), &m, &envelope) {
            Ok(result) => {
                // The durable record is written before this acknowledgement:
                // the proof is the authority's own record of it, never a
                // transport receipt, and a redelivery returns the same proof.
                if let Some(proof) = result.denial_proof {
                    denial_acks.push(DenialAck {
                        sync_mutation_id: m.sync_mutation_id.clone(),
                        proof,
                    });
                }
                match result.outcome {
                    ApplyOutcome::DeadLettered { .. } => rejected.push(m.sync_mutation_id),
                    _ => accepted.push(m.sync_mutation_id),
                }
            }
            Err(_) => {
                // Transient storage failure: no ack, client retries unchanged.
                return write_status_error(
                    stream,
                    500,
                    &AmError::new(ErrorCode::Internal, "transient storage failure"),
                );
            }
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
