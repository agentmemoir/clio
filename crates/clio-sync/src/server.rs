// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the authenticated sync server (
//! `sync_serve`): the public server options and handle, route semantics
//! for push/pull/status, credential material and bank authorization checks,
//! server-side journal sealing via the local DEK provider, and the
//! construction of the bounded serving runtime.
//!
//! Resource bounds live at their owners: head/body caps and the per-read
//! slice in `http` (64 KiB head / 16 MiB body / 5 s slice), the total request
//! deadline default (30 s) and accept loop in `server_conn` (deployment knob
//! `SyncServerOptions::deadline_secs`), and the 8-connection cap in
//! `conn_pool`.
//!
//! ## Owns
//! - `SyncServerOptions` / `SyncServer` / `sync_serve` (public server surface)
//! - Route semantics: `/sync/push`, `/sync/pull`, `/sync/status` handlers
//! - Credential store and bank authorization checks
//! - Server-side journal sealing via the local DEK provider (no peer DEKs)
//!
//! ## Does not own
//! - Connection lifecycle: accept loop, pool slots, refusal mapping
//!   (`server_conn`)
//! - Read bounds and framing (`http`)
//! - Apply matrix / conflict / DLQ semantics (see `apply`)
//! - Cursor format (see `protocol`)
//! - Client-side push/pull loop (see `client`)
//! - DEK/KMS internals (see `clio-store` `dek`)
//!
//! ## Boundary
//!
//! Changes to route semantics or the public server options should normally
//! require changes here. Changes to connection acceptance, slot accounting,
//! or refusal statuses should not require changes here (`server_conn`,
//! `conn_pool`).
//!
//! Keep this module focused on the sync server's routes and public surface.

use crate::apply::{ApplyEngine, ApplyOutcome, apply_mutation, quarantine};
use crate::bind_warning::warn_public_bind;
use crate::clock::now_utc;
use crate::conn_pool::{ConnectionPool, MAX_CONNECTIONS};
use crate::credential::{AuthOutcome, CredentialStore};
use crate::protocol::{
    PullRequest, PullResponse, PushAck, SyncContent, SyncMutation, decode_cursor, encode_cursor,
};
use crate::server_conn::{
    DEFAULT_REQUEST_DEADLINE_SECS, ServeContext, serve_loop, write_json, write_status_error,
};
use clio_store::{DekProvider, Store, SyncJournalRow, SyncStore};
use clio_types::{AmError, ErrorCode};
use serde_json::Value;
use std::collections::HashMap;
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Server construction options.
pub struct SyncServerOptions {
    /// Bind address (`host:port`); port `0` picks an ephemeral high port.
    pub bind: String,
    /// Server-side durable store (the server is a full peer).
    pub store: Arc<dyn Store>,
    /// Sync journal / cursors / conflicts / DLQ view of the same backend.
    pub sync: Arc<dyn SyncStore>,
    /// Local DEK provider used to seal journaled content at rest. DEKs never
    /// leave this node.
    pub kms: Arc<dyn DekProvider>,
    /// Banks this server accepts traffic for.
    pub banks: Vec<String>,
    /// Credential store file; each presented secret resolves to its banks.
    pub token_file: Option<PathBuf>,
    /// Dev mode: profile explicitly marked dev; may relax auth.
    pub dev_mode: bool,
    /// Auto-stop TTL. `None` uses the 30-minute interactive default.
    pub ttl_secs: Option<u64>,
    /// Total request deadline seconds, bounding the *read* of one request
    /// (first accepted byte through the end of the read, plus one check before
    /// the route is dispatched): the deployment raise/lower knob for the 30 s
    /// default (`DEFAULT_REQUEST_DEADLINE_SECS`). `None` uses the default. A
    /// push whose apply overruns is still answered normally; the response write
    /// is bounded per call by the write slice, not by this deadline.
    pub deadline_secs: Option<u64>,
    /// This server's device id (LWW tie-breaker participant).
    pub device_id: String,
}

/// Per-connection serving options shared with `server_conn` (deadline, auth
/// material) and the route handlers below.
pub(crate) struct ServeOptions {
    pub(crate) store: Arc<dyn Store>,
    pub(crate) sync: Arc<dyn SyncStore>,
    pub(crate) kms: Arc<dyn DekProvider>,
    pub(crate) banks: Vec<String>,
    /// Loaded credential store; `None` when no token file was configured.
    pub(crate) credentials: Option<CredentialStore>,
    pub(crate) dev_mode: bool,
    pub(crate) deadline_secs: u64,
    pub(crate) device_id: String,
    /// One lock per authorized bank. The server runs a worker per accepted
    /// connection, so pushes for the same bank can otherwise interleave inside
    /// the apply path, which is a check-then-act sequence over the item, the
    /// apply-guard, and the journal. Serializing per bank (never globally, so
    /// two banks never wait on each other) makes the existing dedupe checks
    /// and last-writer-wins resolution mean what they say.
    pub(crate) bank_locks: BankLocks,
}

/// Per-bank apply locks: a lock is created on first use for a bank, so a bank
/// name that never appears in a push costs nothing.
#[derive(Default)]
pub(crate) struct BankLocks {
    locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

impl BankLocks {
    /// The lock for `bank`, created on first use. The caller locks it in its
    /// own scope so the returned handle and its guard share one lifetime.
    fn for_bank(&self, bank: &str) -> Arc<Mutex<()>> {
        let mut locks = self.locks.lock().expect("bank lock map poisoned");
        Arc::clone(
            locks
                .entry(bank.to_owned())
                .or_insert_with(|| Arc::new(Mutex::new(()))),
        )
    }
}

/// A running sync server handle.
pub struct SyncServer {
    /// Bound address (resolved port for ephemeral binds).
    addr: String,
    stop: Arc<AtomicBool>,
}

impl SyncServer {
    /// Bound address including the resolved ephemeral port.
    #[must_use]
    pub fn addr(&self) -> &str {
        &self.addr
    }

    /// Stop the accept loop; subsequent calls are no-ops.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// Start the sync server on a background thread.
///
/// # Errors
/// Bind failure, or auth material missing outside dev mode.
pub fn sync_serve(options: SyncServerOptions) -> Result<SyncServer, AmError> {
    let token_file = options.token_file.as_deref();
    let credentials = token_file.map(CredentialStore::load).transpose()?;
    if credentials.is_none() && !options.dev_mode {
        return Err(AmError::new(
            ErrorCode::Forbidden,
            "sync_serve requires auth material outside dev mode (refusing anonymous bind)",
        ));
    }
    let listener = TcpListener::bind(&options.bind)
        .map_err(|e| AmError::new(ErrorCode::Internal, format!("bind failed: {e}")))?;
    listener
        .set_nonblocking(true)
        .expect("listener nonblocking setup");
    let local = listener.local_addr().expect("listener local addr");
    warn_public_bind(local);
    let addr = local.to_string();
    let stop = Arc::new(AtomicBool::new(false));
    let ctx = ServeContext {
        listener,
        stop: Arc::clone(&stop),
        options: Arc::new(ServeOptions {
            store: options.store,
            sync: options.sync,
            kms: options.kms,
            banks: options.banks,
            credentials,
            dev_mode: options.dev_mode,
            deadline_secs: options
                .deadline_secs
                .unwrap_or(DEFAULT_REQUEST_DEADLINE_SECS),
            device_id: options.device_id,
            bank_locks: BankLocks::default(),
        }),
        ttl_secs: options.ttl_secs.unwrap_or(crate::SERVE_TTL_DEFAULT_SECS),
        started: Instant::now(),
        pool: Arc::new(ConnectionPool::new(MAX_CONNECTIONS)),
    };
    std::thread::Builder::new()
        .name("clio-sync-serve".into())
        .spawn(move || serve_loop(&ctx))
        .expect("serve spawn");
    Ok(SyncServer { addr, stop })
}

pub(crate) fn bank_authorized(options: &ServeOptions, auth: &AuthOutcome, bank: &str) -> bool {
    auth.allows(bank) && options.banks.iter().any(|b| b == bank)
}

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
        match apply_mutation(&engine, &m, &envelope) {
            Ok(outcome) => match outcome {
                ApplyOutcome::DeadLettered { .. } => rejected.push(m.sync_mutation_id),
                _ => accepted.push(m.sync_mutation_id),
            },
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
        })
        .expect("serialize push ack"),
    )
}

pub(crate) fn handle_pull(
    options: &ServeOptions,
    stream: TcpStream,
    body: &str,
    auth: &AuthOutcome,
) -> Result<(), AmError> {
    let request: PullRequest = serde_json::from_str(body)
        .map_err(|e| AmError::new(ErrorCode::InvalidArgument, format!("bad pull body: {e}")))?;
    if !bank_authorized(options, auth, &request.bank) {
        return write_status_error(
            stream,
            403,
            &AmError::new(ErrorCode::Forbidden, "bank not authorized"),
        );
    }
    let since = if request.cursor.is_empty() {
        0
    } else {
        decode_cursor(&request.cursor, &request.bank)?
    };
    let rows = options
        .sync
        .journal_list_since(&request.bank, since, request.limit)?;
    let mutations = rows
        .iter()
        .map(|row| journal_row_to_mutation(options.kms.as_ref(), row))
        .collect::<Result<Vec<_>, AmError>>()?;
    let last_seq = rows.last().map_or(since, |r| r.seq);
    let next_cursor = encode_cursor(&request.bank, last_seq);
    write_json(
        stream,
        200,
        &serde_json::to_value(PullResponse {
            mutations,
            next_cursor,
            has_more: rows.len() >= request.limit,
        })
        .expect("serialize pull response"),
    )
}

fn journal_row_to_mutation(
    kms: &dyn DekProvider,
    row: &SyncJournalRow,
) -> Result<SyncMutation, AmError> {
    let sealed = row.content_sealed.as_deref().unwrap_or_default();
    if row.client_encrypted {
        if sealed.is_empty() {
            return Err(AmError::new(
                ErrorCode::Internal,
                format!(
                    "client-encrypted journal row {} has no ciphertext",
                    row.sync_mutation_id
                ),
            ));
        }
        return serde_json::from_str(sealed)
            .map_err(|e| AmError::new(ErrorCode::Internal, format!("bad cipher row: {e}")));
    }
    // A journal row sealed by a KMS-less writer stores no content. Serve empty
    // logical content with its routing metadata rather than failing the whole
    // pull page on an unsealed row.
    let content =
        if sealed.is_empty() {
            SyncContent::Plain(serde_json::json!({}))
        } else {
            let envelope: clio_store::CiphertextEnvelope = serde_json::from_str(sealed)
                .map_err(|e| AmError::new(ErrorCode::Internal, format!("bad sealed row: {e}")))?;
            let bytes = kms.decrypt(&envelope)?;
            SyncContent::Plain(serde_json::from_slice::<Value>(&bytes).map_err(|e| {
                AmError::new(ErrorCode::Internal, format!("bad journal content: {e}"))
            })?)
        };
    Ok(SyncMutation {
        sync_mutation_id: row.sync_mutation_id.clone(),
        bank: row.bank_id.clone(),
        entity_kind: row.entity_kind.clone(),
        entity_id: row.entity_id.clone(),
        op: row.op.clone(),
        transaction_time: row.transaction_time.clone(),
        device_id: row.device_id.clone(),
        content,
        content_hash: row.content_hash.clone(),
        admission_receipt: row
            .admission_receipt
            .as_ref()
            .and_then(|s| serde_json::from_str(s).ok()),
    })
}

pub(crate) fn handle_status(
    options: &ServeOptions,
    stream: TcpStream,
    auth: &AuthOutcome,
) -> Result<(), AmError> {
    let mut banks = Vec::new();
    for bank in options.banks.iter().filter(|b| auth.allows(b)) {
        banks.push(serde_json::json!({
            "bank_id": bank,
            "journal_seq": options.sync.journal_max_seq(bank).unwrap_or(0),
            "dead_letter_open": options
                .sync
                .dead_letter_count(bank, clio_store::DeadLetterStatus::Open)
                .unwrap_or(0),
            "conflicts": options.sync.conflict_count(bank).unwrap_or(0),
        }));
    }
    write_json(
        stream,
        200,
        &serde_json::json!({
            "role": "server",
            "device_id": options.device_id,
            "dev_mode": options.dev_mode,
            "banks": banks,
            "timestamp": now_utc(),
        }),
    )
}

#[cfg(test)]
#[path = "server_connection_cap_tests.rs"]
mod server_connection_cap_tests;
#[cfg(test)]
#[path = "server_error_tests.rs"]
mod server_error_tests;
#[cfg(test)]
#[path = "server_push_payload_measurement_tests.rs"]
mod server_push_payload_measurement_tests;
#[cfg(test)]
#[path = "server_request_bounds_tests.rs"]
mod server_request_bounds_tests;
#[cfg(test)]
#[path = "server_scope_tests.rs"]
pub(crate) mod server_scope_tests;
#[cfg(test)]
#[path = "server_tests.rs"]
pub(crate) mod server_tests;
