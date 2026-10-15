// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the verified compliance erasure workflow
//! (`erase_request`): authorization, confirmation gating, DEK
//! destruction, derived-structure propagation, and the audit outcome.
//!
//! ## Owns
//! - `erase_request` argument validation and the harness authorisation gate
//! - The confirm gate (unconfirmed calls never mutate)
//! - Orchestrating the store crypto-shred transaction and `MemTree` propagation
//! - Recording the durable denial in the owner-only erasure journal strictly
//!   before the store can destroy the subject key
//! - Holding the journal's placement lock across the denial and the key
//!   destruction, so a restore placement cannot interleave between them
//! - Content-free erasure telemetry, carrying the presenting credential's label
//!   when a credential presented the request
//!
//! ## Does not own
//! - The persistence transaction itself (`clio-store::ComplianceStore`)
//! - Dirty-path refresh algorithm (`clio-write::MemTreeMaint`)
//! - The journal's model, checks, or owner-only file mechanics
//!   (`erasure_journal`, `erasure_journal_file`)
//! - MCP argument decoding and schema publication (`clio-mcp`)
//!
//! ## Boundary
//!
//! Changes to the erasure workflow or its outcome shape should normally require
//! changes here. Changes to backend SQL or `MemTree` refresh should not.
//!
//! Keep this module focused on the compliance erasure workflow.

use clio_store::{ComplianceStore, SubjectEraseSpec, sha256_hex};
use clio_types::{Actor, AmError, ErrorCode, OpContext};
use clio_write::MemTreeMaint;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::{Arc, Mutex};

/// Default machine reason code recorded on every compliance tombstone.
pub const DEFAULT_REASON_CODE: &str = "COMPLIANCE_CRYPTO_SHRED";

/// Built-in deployment salt used when `CLIO_COMPLIANCE_SALT` is unset.
const DEFAULT_SALT: &str = "clio-compliance-tombstone-v1";

/// Resolve the tombstone salt from an explicit raw deployment value.
///
/// A configured value wins; otherwise the built-in default keeps single-host
/// deployments working out of the box.
#[must_use]
pub fn tombstone_salt_from(raw: Option<String>) -> Vec<u8> {
    raw.map_or_else(|| DEFAULT_SALT.as_bytes().to_vec(), String::into_bytes)
}

/// Deployment salt for one-way tombstone hashing (`CLIO_COMPLIANCE_SALT`).
#[must_use]
pub fn tombstone_salt() -> Vec<u8> {
    tombstone_salt_from(std::env::var("CLIO_COMPLIANCE_SALT").ok())
}

/// Domain string keyed by the deployment salt so a fingerprint reveals nothing
/// about the salt itself.
const SALT_FINGERPRINT_DOMAIN: &str = "clio-compliance-salt-fingerprint-v1";

/// A non-reversible fingerprint of the deployment tombstone salt.
///
/// The archive manifest and the erasure journal each record this value, so a
/// restore can refuse an archive whose subject ids were hashed under a
/// different salt than the live authority recorded. It is a one-way digest of
/// a fixed domain string keyed by the salt, so it neither reveals the salt nor
/// changes when the salt does not.
#[must_use]
pub fn tombstone_salt_fingerprint() -> String {
    sha256_hex(&tombstone_salt(), SALT_FINGERPRINT_DOMAIN)
}

/// Inputs for the in-process `erase_request` tool.
pub struct EraseArgs<'a> {
    /// Compliance persistence boundary.
    pub store: &'a dyn ComplianceStore,
    /// Owner-only erasure-journal path: the durable denial of the subject key
    /// is recorded here before any key loss. Never a path a restore replaces.
    pub journal_path: &'a Path,
    /// Live `MemTree` engine for derived-summary propagation (optional).
    pub maint: Option<&'a Arc<MemTreeMaint>>,
    /// Bank / actor context (actor must be privileged).
    pub ctx: &'a OpContext,
    /// Data-subject id whose DEK is destroyed.
    pub subject_id: &'a str,
    /// Legal basis for the request.
    pub legal_basis: &'a str,
    /// Caller compliance tracking id.
    pub request_id: &'a str,
    /// Optional machine reason code (defaults to [`DEFAULT_REASON_CODE`]).
    pub reason_code: Option<&'a str>,
    /// `true` performs the erasure; `false` is rejected (no dry-run path).
    pub confirm: bool,
    /// Deployment salt for the one-way tombstone hashes.
    pub salt: &'a [u8],
    /// Erasure timestamp (UTC text).
    pub timestamp: &'a str,
    /// Content-free telemetry sink.
    pub sink: &'a dyn EraseSink,
}

/// Content-free erasure telemetry (never carries plaintext subject or item ids).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EraseEvent {
    /// Always `compliance_erase`.
    pub operation: String,
    /// Bank isolation key.
    pub bank: String,
    /// Privileged actor that authorized the erasure.
    pub actor: String,
    /// Label of the credential that presented the erasure; absent when no
    /// credential did. Never a secret and never a digest of one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub principal: Option<String>,
    /// Erasure timestamp.
    pub timestamp: String,
    /// Caller request id.
    pub request_id: String,
    /// One-way subject hash.
    pub subject_hash: String,
    /// Number of items erased.
    pub item_count: usize,
    /// Machine reason code.
    pub reason_code: String,
    /// Legal basis.
    pub legal_basis: String,
}

/// Sink for erasure telemetry.
pub trait EraseSink: std::fmt::Debug {
    /// Record one event.
    fn emit(&self, event: EraseEvent);
}

/// The credential label an event carries, or `None` on an unbound transport.
///
/// Recorded as the label was resolved, so an event names the credential and
/// derives nothing from its secret.
fn principal_label(ctx: &OpContext) -> Option<String> {
    ctx.principal().map(|label| label.as_str().to_owned())
}

#[cfg(test)]
#[path = "erase_tests.rs"]
mod erase_tests;

#[cfg(test)]
#[path = "erase_failure_tests.rs"]
mod erase_failure_tests;

/// In-memory sink for tests.
#[derive(Debug, Default)]
pub struct EraseVecSink {
    events: Mutex<Vec<EraseEvent>>,
}

impl EraseVecSink {
    /// Snapshot of recorded events.
    #[must_use]
    pub fn events(&self) -> Vec<EraseEvent> {
        self.events.lock().expect("erase sink poisoned").clone()
    }
}

impl EraseSink for EraseVecSink {
    fn emit(&self, event: EraseEvent) {
        self.events.lock().expect("erase sink poisoned").push(event);
    }
}

/// Structured erasure status response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EraseOutcome {
    /// `COMPLETED` (all derived structures purged) or `PROCESSING` (key
    /// destroyed but propagation must be retried).
    pub status: String,
    /// Caller request id.
    pub request_id: String,
    /// One-way subject hash.
    pub subject_hash: String,
    /// Erasure timestamp.
    pub shredded_at: String,
    /// Number of erased items.
    pub item_count: usize,
    /// Tombstone rows written.
    pub tombstones_written: usize,
    /// Dense vector rows purged.
    pub vectors_purged: usize,
    /// Association edges purged.
    pub assoc_edges_purged: usize,
    /// Derived `MemTree` nodes refreshed after detaching erased leaves.
    pub memtree_nodes_refreshed: usize,
    /// SPO triple rows deleted, with the sync-journal and dead-letter payload
    /// copies of the same triples. A triple row keeps its plaintext
    /// `subject` + `predicate` identity key after the DEK is destroyed, so the
    /// erasure is only complete once the row is gone.
    pub triples_purged: usize,
    /// Derived hub-distilled items purged because they referenced erased data.
    pub derived_items_purged: usize,
    /// Persona context rows rendered unreadable by this erasure (bank-scoped
    /// persona context attribution; 0 when unknown during `PROCESSING`).
    pub persona_context_rows_purged: usize,
}

/// In-process `erase_request(subject_id, legal_basis, request_id, confirm)`.
///
/// AUTHORISATION: only `harness` or `system` actors may run this; agent/user
/// callers get [`ErrorCode::Forbidden`]. Confirmation is mandatory: without
/// `confirm=true` the call is rejected with zero writes and no key destruction.
///
/// The DEK is destroyed before the derived-structure purge so the content is
/// unrecoverable even if propagation fails; a propagation failure then reports
/// `status: PROCESSING` instead of a false `COMPLETED`. The denial is durably
/// recorded in the owner-only journal strictly before the store can destroy
/// the key: a failed journal write aborts with no store effect and no key loss.
/// The denial and the key destruction run under the journal's exclusive
/// placement lock, so a restore placement cannot interleave between them.
///
/// # Errors
/// Validation, authorisation, confirmation, journal, DEK, or storage failures.
pub fn erase_request(args: &EraseArgs<'_>) -> Result<EraseOutcome, AmError> {
    validate(args)?;
    authorize(args.ctx.actor)?;
    if !args.confirm {
        return Err(AmError::new(
            ErrorCode::Forbidden,
            "erase_request requires confirm=true; unconfirmed calls never mutate",
        ));
    }
    let reason_code = args.reason_code.unwrap_or(DEFAULT_REASON_CODE);
    let spec = SubjectEraseSpec {
        bank_id: args.ctx.bank.as_str(),
        subject_id: args.subject_id,
        request_id: args.request_id,
        legal_basis: args.legal_basis,
        reason_code,
        deleted_at: args.timestamp,
        salt: args.salt,
    };
    // Hold the placement/erasure coordination lock across the denial and the
    // key destruction, so a restore placement cannot interleave between them.
    // A lock-acquire failure fails closed: it returns the `AmError` the hold
    // produced, the same `?` mapping the `record_denial` call below uses. The
    // hold drops with this scope, after `erase_subject` returns.
    let _hold = crate::restore_guard::JournalHold::acquire(args.journal_path)?;
    // Denial precedes key loss: the journal write fsyncs the file and its
    // directory, and a failure here aborts before `erase_subject` runs.
    let subject_hash = sha256_hex(args.salt, args.subject_id);
    crate::erasure_journal::record_denial(
        args.journal_path,
        &subject_hash,
        &[args.ctx.bank.as_str().to_owned()],
    )?;
    match args.store.erase_subject(&spec) {
        Ok(outcome) => {
            let refreshed = propagate(args.maint, spec.bank_id, &outcome.item_ids);
            args.sink.emit(EraseEvent {
                operation: "compliance_erase".into(),
                bank: spec.bank_id.into(),
                actor: args.ctx.actor.as_str().into(),
                principal: principal_label(args.ctx),
                timestamp: args.timestamp.into(),
                request_id: spec.request_id.into(),
                subject_hash: outcome.subject_hash.clone(),
                item_count: outcome.item_ids.len(),
                reason_code: reason_code.into(),
                legal_basis: spec.legal_basis.into(),
            });
            Ok(EraseOutcome {
                status: "COMPLETED".into(),
                request_id: spec.request_id.into(),
                subject_hash: outcome.subject_hash,
                shredded_at: spec.deleted_at.into(),
                item_count: outcome.item_ids.len(),
                tombstones_written: outcome.tombstones_written,
                vectors_purged: outcome.vectors_purged,
                assoc_edges_purged: outcome.assoc_edges_purged,
                memtree_nodes_refreshed: refreshed,
                triples_purged: outcome.triples_purged,
                derived_items_purged: outcome.derived_items_purged,
                persona_context_rows_purged: outcome.persona_context_rows_purged,
            })
        }
        Err(err) => {
            // Partial completion: the DEK may already be destroyed (irreversible).
            // Never report a false failure that invites a second subject-wide
            // rewrite; surface PROCESSING so the caller retries propagation.
            if args
                .store
                .subject_dek_destroyed(args.subject_id)
                .unwrap_or(false)
            {
                let subject_hash = sha256_hex(args.salt, args.subject_id);
                args.sink.emit(EraseEvent {
                    operation: "compliance_erase".into(),
                    bank: spec.bank_id.into(),
                    actor: args.ctx.actor.as_str().into(),
                    principal: principal_label(args.ctx),
                    timestamp: args.timestamp.into(),
                    request_id: spec.request_id.into(),
                    subject_hash: subject_hash.clone(),
                    item_count: 0,
                    reason_code: reason_code.into(),
                    legal_basis: spec.legal_basis.into(),
                });
                Ok(EraseOutcome {
                    status: "PROCESSING".into(),
                    request_id: spec.request_id.into(),
                    subject_hash,
                    shredded_at: spec.deleted_at.into(),
                    item_count: 0,
                    tombstones_written: 0,
                    vectors_purged: 0,
                    assoc_edges_purged: 0,
                    memtree_nodes_refreshed: 0,
                    triples_purged: 0,
                    derived_items_purged: 0,
                    persona_context_rows_purged: 0,
                })
            } else {
                Err(err)
            }
        }
    }
}

/// Detach erased leaves and recompute `MemTree` ancestors through the existing
/// dirty-path engine; returns the number of refreshed nodes.
fn propagate(maint: Option<&Arc<MemTreeMaint>>, bank_id: &str, item_ids: &[String]) -> usize {
    let Some(maint) = maint else {
        return 0;
    };
    maint.detach_erased(bank_id, item_ids);
    maint.refresh_bank(bank_id).len()
}

/// Only harness/system actors carry compliance erasure privilege.
fn authorize(actor: Actor) -> Result<(), AmError> {
    match actor {
        Actor::Harness | Actor::System => Ok(()),
        Actor::Agent | Actor::User => Err(AmError::new(
            ErrorCode::Forbidden,
            "erase_request requires a harness or system actor with compliance privilege",
        )),
    }
}

fn validate(args: &EraseArgs<'_>) -> Result<(), AmError> {
    if args.ctx.bank.as_str().is_empty() {
        return Err(AmError::new(ErrorCode::MissingBank, "bank is required"));
    }
    if args.subject_id.trim().is_empty() {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "subject_id is required",
        ));
    }
    if args.legal_basis.trim().is_empty() {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "legal_basis is required",
        ));
    }
    if args.request_id.trim().is_empty() {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "request_id is required",
        ));
    }
    Ok(())
}
