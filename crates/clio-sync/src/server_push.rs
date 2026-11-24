// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the sync server's push route: request
//! decoding, per-mutation bank authorization, the credential authority the
//! denial-carrier gate reads, the per-bank apply lock, the guarded apply of
//! every mutation, the dead-letter quarantine of refused mutations, and the
//! acknowledgement that carries each denial operation's durable proof.
//!
//! ## Owns
//! - `handle_push` and the push acknowledgement body it writes
//! - The authorization refusals (the envelope bank and each mutation) and
//!   their quarantine placement
//! - The credential authority (`PushCarrierAuthority`) the denial-carrier gate
//!   reads, and the sealed subject check the apply path cannot make
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

use crate::apply::{ApplyEngine, ApplyOutcome, apply_mutation_guarded_relaying, quarantine};
use crate::credential::AuthOutcome;
use crate::crypto::SyncCipherKey;
use crate::denial::DenialAck;
use crate::denial_guard::CarrierAuthority;
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

/// The presented credential's authority over a denial carrier on this route.
///
/// It reads the actor's erasure privilege and the per-bank reach the route
/// already applies to every mutation, and hands both to the carrier gate.
struct PushCarrierAuthority<'a> {
    options: &'a ServeOptions,
    auth: &'a AuthOutcome,
}

impl CarrierAuthority for PushCarrierAuthority<'_> {
    fn may_record_denial(&self) -> bool {
        self.auth.may_record_denial()
    }

    fn bank_authorized(&self, bank: &str) -> bool {
        bank_authorized(self.options, self.auth, bank)
    }
}

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
#[allow(clippy::too_many_arguments)]
fn apply_and_record(
    engine: &ApplyEngine<'_>,
    options: &ServeOptions,
    auth: &AuthOutcome,
    bank: &str,
    m: &SyncMutation,
    envelope: &str,
    accepted: &mut Vec<String>,
    rejected: &mut Vec<String>,
    denial_acks: &mut Vec<DenialAck>,
) -> Result<(), AmError> {
    let denial = options.denial.as_deref();
    let authority = PushCarrierAuthority { options, auth };
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
            apply_mutation_guarded_relaying(engine, denial, Some(&authority), &opened, envelope, m)?
        }
        None => apply_mutation_guarded_relaying(engine, denial, Some(&authority), m, envelope, m)?,
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
    // Every outcome is matched by name: a stale row was never applied, so it
    // must never be acknowledged as accepted, or the sender's watermark moves
    // past a row this node dropped and the row is lost for the whole mesh.
    match &result.outcome {
        ApplyOutcome::Applied | ApplyOutcome::Duplicate | ApplyOutcome::ConflictLoser => {
            accepted.push(m.sync_mutation_id.clone());
        }
        ApplyOutcome::Stale => {
            crate::apply::quarantine_stale(engine, bank, m, envelope)?;
            rejected.push(m.sync_mutation_id.clone());
        }
        ApplyOutcome::DeadLettered { .. } => rejected.push(m.sync_mutation_id.clone()),
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

/// The bank set one push must hold before it applies anything, sorted and
/// deduplicated.
///
/// The envelope bank is here because a refused mutation — one for a bank this
/// credential cannot reach, or one carrying content the route refuses — is
/// quarantined under it. Each mutation's own bank is here because that
/// mutation's item row, apply guard, and journal effects land in that bank,
/// whatever bank the envelope names. Each covered bank of a denial carrier is
/// here because the serve marker for that bank is written when the carrier
/// applies.
///
/// A carrier's covered banks are read from the CONTENT THIS PEER OPENS, not
/// from the ciphertext: a sealed carrier is refused by the plaintext decode, so
/// a set built from the row as it arrived would miss every bank whose marker
/// the apply path then writes unlocked. `sync_key` is the key the apply path
/// opens with, so both read the same banks.
///
/// The shared dead-letter status row (`__status`, a compile-time constant) is
/// deliberately NOT locked: it is one cross-bank slot written by a keyed
/// upsert, not a per-bank read-then-write item row, and locking that single
/// slot would serialize every push on the node. So this is not "every bank a
/// push can write"; it is every authorized per-bank item state a mutation
/// writes, plus every covered bank of a carrier it can record.
///
/// Sorting and deduplicating the set means every push acquires its locks in one
/// global order, so two pushes that overlap on any bank cannot deadlock.
fn banks_to_lock(
    envelope: &str,
    mutations: &[SyncMutation],
    sync_key: Option<&SyncCipherKey>,
) -> Vec<String> {
    let mut banks = vec![envelope.to_owned()];
    for m in mutations {
        banks.push(m.bank.clone());
        // The effective mutation for carrier inspection: the content this peer
        // opens under its own key, decoded exactly as `apply_and_record` opens
        // it, so the lock set and the writes it guards cannot disagree.
        let opened;
        let effective = match sync_key.and_then(|key| crate::denial_guard::sealed_content(key, m)) {
            Some(content) => {
                opened = SyncMutation {
                    content: SyncContent::Plain(content),
                    ..m.clone()
                };
                &opened
            }
            None => m,
        };
        if let Ok(Some(carrier)) = crate::denial::denial_carrier(effective) {
            banks.extend(carrier.covered_banks().iter().cloned());
        }
    }
    banks.sort_unstable();
    banks.dedup();
    banks
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
    // connection. A mutation writes its own bank, not the envelope bank, so
    // this push holds a lock for the envelope bank and for every authorized
    // bank a mutation writes item state to, plus every covered bank of a
    // carrier it can record: without that, two pushes naming different envelope
    // banks but reaching the same bank would race on that bank's item row and
    // an older last-writer-wins stamp could land last. Several locks are taken
    // in one global sorted order, so overlapping pushes cannot deadlock. The
    // shared `__status` dead-letter slot is the one write deliberately not
    // covered: it is a single cross-bank keyed upsert, and locking it would
    // serialize every push on the node.
    //
    // Banks the credential cannot reach are filtered out first. A mutation for
    // one is quarantined under the envelope bank and never writes its own, so
    // locking it would stall unrelated pushes and grow the lock map for no
    // reason. `bank_authorized` requires both that the credential allows the
    // bank and that the server lists it, so this filter bounds the lock map to
    // the server's fixed configured bank list and no separate cap is needed.
    //
    // A poisoned lock means a previous apply panicked mid-push; the apply
    // steps are transactional, so the next push proceeds.
    //
    // Both vectors outlive the apply loop and the ack write: the handles are
    // named without a leading underscore because the guards are built from
    // them, and the guards are named with one because holding them is their
    // whole purpose.
    let lock_handles: Vec<_> = banks_to_lock(&bank, &mutations, options.sync_key.as_deref())
        .into_iter()
        .filter(|name| bank_authorized(options, auth, name))
        .map(|name| options.bank_locks.for_bank(&name))
        .collect();
    let _guards: Vec<_> = lock_handles
        .iter()
        .map(|handle| {
            handle
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        })
        .collect();
    let engine = ApplyEngine {
        store: options.store.as_ref(),
        sync: options.sync.as_ref(),
        device_id: options.device_id.clone(),
        kms: Some(options.kms.as_ref()),
        clock: &crate::clock::now_epoch,
        // The serve options resolved the operator's freshness window, so this
        // route applies the same bound the pushing client holds.
        max_envelope_skew_secs: options.max_envelope_skew_secs,
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
        // A denial carrier answers to the credential gate inside the apply
        // path, where every recorded denial passes: a mutation that is not one
        // passes straight through.
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
            auth,
            &bank,
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

#[cfg(test)]
#[path = "server_push_stale_tests.rs"]
mod server_push_stale_tests;

#[cfg(test)]
#[path = "server_push_sealed_tests.rs"]
mod server_push_sealed_tests;

#[cfg(test)]
#[path = "server_push_lock_tests.rs"]
mod server_push_lock_tests;
