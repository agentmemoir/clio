// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the sync server's pull route: decoding the
//! request cursor, refusing a bank a recorded denial covers, reading one
//! bounded journal page, converting journal rows into wire mutations, and
//! writing the page response.
//!
//! ## Owns
//! - Pull page assembly: the row cap (`PULL_PAGE_LIMIT`) and the serialized
//!   byte budget (`PULL_RESPONSE_BYTE_BUDGET`), plus `has_more` and the resume
//!   cursor, both derived from the rows actually assembled
//! - Journal row to wire mutation conversion, including client-encrypted rows
//!   and rows sealed by this node's KMS
//! - The pull refusal status mapping: a bank a recorded denial covers answers
//!   `403`, and an unreadable authority answers `500`
//!
//! ## Does not own
//! - Credential resolution and bank authorization (`credential`, `server`)
//! - Denial semantics and the guards over them (`denial`, `denial_guard`)
//! - Cursor format (`protocol`)
//! - Request framing, read bounds, and the response write (`http`,
//!   `server_conn`)
//!
//! ## Boundary
//!
//! Changes to what one pull page contains should normally require changes
//! here. Changes to route dispatch, connection lifecycle, or another route's
//! refusal mapping should not.
//!
//! Keep this module focused on the pull route.

use crate::credential::AuthOutcome;
use crate::protocol::{
    PullRequest, PullResponse, SyncContent, SyncMutation, decode_cursor, encode_cursor,
};
use crate::server::{ServeOptions, bank_authorized};
use crate::server_conn::{write_json, write_status_error};
use clio_store::{DekProvider, SyncJournalRow};
use clio_types::{AmError, ErrorCode};
use std::net::TcpStream;

/// The status a pull refusal answers with: a bank a recorded denial covers is a
/// `403`; an unreadable authority is the server's own failure (`500`).
fn pull_refusal_status(err: &AmError) -> u16 {
    if err.code == ErrorCode::ErasedSubject {
        403
    } else {
        500
    }
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
    let limit = request.limit.clamp(1, crate::PULL_PAGE_LIMIT);
    // The bank guard: a bank a recorded denial covers is refused whole, and an
    // unreadable authority refuses the pull instead of serving it. A journal
    // row carries no readable subject, so a covered bank cannot be resolved
    // row by row; unrelated banks still serve normally.
    //
    // The check and the read happen under the same per-bank lock the push
    // route holds, so a denial for this bank cannot be recorded between them
    // and still answered with pre-denial rows: while the lock is held the
    // denial is either already recorded (the check refuses) or lands after the
    // read (the page is ordered before it). The read is re-checked under the
    // same lock because a carrier pushed under another bank may cover this one
    // and record its marker without ever taking this bank's lock.
    let rows = {
        let bank_lock = options.bank_locks.for_bank(&request.bank);
        let _bank_guard = bank_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Err(err) =
            crate::denial_guard::pull_refusal(options.denial.as_deref(), &request.bank)
        {
            return write_status_error(stream, pull_refusal_status(&err), &err);
        }
        let rows = options
            .sync
            .journal_list_since(&request.bank, since, limit)?;
        if let Err(err) =
            crate::denial_guard::pull_refusal(options.denial.as_deref(), &request.bank)
        {
            return write_status_error(stream, pull_refusal_status(&err), &err);
        }
        rows
    };
    // The row cap bounds the number of rows, not their bytes: one row may carry
    // a large sealed payload, and the response is serialized whole before it is
    // written. Assemble the page against a byte budget, always keeping the
    // first row so a client makes progress and resumes from its cursor.
    let mut mutations = Vec::new();
    let mut bytes = 0usize;
    let mut last_seq = since;
    for row in &rows {
        let mutation = journal_row_to_mutation(options.kms.as_ref(), row)?;
        let size = serde_json::to_vec(&mutation)
            .map_err(|e| AmError::new(ErrorCode::Internal, format!("measure pull row: {e}")))?
            .len();
        if !mutations.is_empty() && bytes.saturating_add(size) > crate::PULL_RESPONSE_BYTE_BUDGET {
            break;
        }
        bytes = bytes.saturating_add(size);
        mutations.push(mutation);
        last_seq = row.seq;
    }
    // More rows remain when the budget held some back, or when the store filled
    // the row cap and may hold rows past it.
    let has_more = mutations.len() < rows.len() || rows.len() >= limit;
    let next_cursor = encode_cursor(&request.bank, last_seq);
    write_json(
        stream,
        200,
        &serde_json::to_value(PullResponse {
            mutations,
            next_cursor,
            has_more,
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
    let content = if sealed.is_empty() {
        SyncContent::Plain(serde_json::json!({}))
    } else {
        SyncContent::Plain(crate::journal_row::open_journal_content(row, sealed, kms)?)
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

#[cfg(test)]
#[path = "server_pull_tests.rs"]
mod server_pull_tests;
