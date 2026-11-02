// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the sync client:
//! `sync_push` / `sync_pull` / bidirectional modes over the HTTP transport,
//! the durable cursor policy (advance only after durable apply or ack-skip),
//! and the masked `sync_status` report.
//!
//! ## Owns
//! - Outgoing change-feed packaging into `sync_payload` batches
//! - Pull apply loop under the optional erasure-denial authority, with the
//!   never-advance-past-failures cursor policy
//! - Bidirectional mode (documented order: pull then push)
//! - `sync_status` surface (device id, cursors, pending, conflicts, DLQ)
//! - Minimal HTTP/1.1 JSON client for the server transport
//!
//! ## Does not own
//! - Opening, decrypting, resolving, or restoring one pulled row
//!   (`client_pull_row`)
//! - Apply matrix / conflict / DLQ semantics (see `apply`)
//! - Server transport (`server`)
//! - Persistence of cursors (`clio-store` `SyncStore`)
//!
//! ## Boundary
//!
//! Changes to client modes, cursor policy, or the status shape should normally
//! require changes here. Changes to how a pulled row is opened or restored
//! should not require changes here, and changes to apply semantics should not
//! either.
//!
//! Keep this module focused on the sync client.

use crate::apply::{ApplyEngine, ApplyOutcome, apply_mutation_guarded};
use crate::client_feed::push_bank;
pub(crate) use crate::client_pull_row::decrypt_for_apply;
use crate::http::post_json;
use crate::protocol::{BankSyncStatus, PullRequest, PullResponse, SyncStatusReport};
use clio_store::{DeadLetterStatus, ErasureDenial, Store, SyncStore};
use clio_types::{AmError, ErrorCode};
use std::sync::Arc;

/// Client construction options.
pub struct SyncClientConfig {
    /// This node's device id.
    pub device_id: String,
    /// Local durable store.
    pub store: Arc<dyn Store>,
    /// Sync journal / cursors / conflicts / DLQ view of the same backend.
    pub sync: Arc<dyn SyncStore>,
    /// Remote endpoint `host:port` (push works without any local serve).
    pub remote: Option<String>,
    /// Bearer token for the remote (masked on status).
    pub token: Option<String>,
    /// Optional client-side encryption keys, newest first: content leaves the
    /// node sealed with the newest key and the server stores opaque bytes.
    /// An empty list means encryption off.
    pub cipher: Vec<crate::crypto::SyncCipherKey>,
}

/// One sync operation mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncModes {
    /// Send local mutations since `push_cursor`.
    Push,
    /// Fetch/apply remote mutations since `pull_cursor`.
    Pull,
    /// Pull then push in one operator action.
    Bidirectional,
}

impl SyncModes {
    /// Parse the mode argument (`push` | `pull` | `bidirectional`).
    ///
    /// # Errors
    /// Unknown mode.
    pub fn parse(raw: &str) -> Result<Self, AmError> {
        match raw {
            "push" => Ok(Self::Push),
            "pull" => Ok(Self::Pull),
            "bidirectional" => Ok(Self::Bidirectional),
            other => Err(AmError::new(
                ErrorCode::InvalidArgument,
                format!("unknown sync mode: {other}"),
            )),
        }
    }
}

/// Push local mutations for the given banks (default: all banks on device).
///
/// # Errors
/// Transport, auth, or transient storage failures (cursors unchanged).
pub fn push(cfg: &SyncClientConfig, banks: &[String]) -> Result<Vec<BankSyncStatus>, AmError> {
    let remote = required_remote(cfg)?;
    let mut out = Vec::new();
    for bank in banks {
        out.push(push_bank(cfg, &remote, bank)?);
    }
    Ok(out)
}

/// Pull remote mutations and apply them (pull mode) with no erasure-denial
/// authority: the documented no-journal deployment. A deployment holding a
/// denial journal passes it to [`pull_with_denial`] instead.
pub fn pull(cfg: &SyncClientConfig, banks: &[String]) -> Result<Vec<BankSyncStatus>, AmError> {
    pull_with_denial(cfg, banks, None)
}

/// Pull remote mutations and apply them under the deployment's erasure-denial
/// authority: every pulled mutation passes the guarded apply entry before it
/// can write, a denied subject is dead-lettered, and an unreadable authority refuses.
///
/// # Errors
/// Transport, auth, or transient storage failures, or a policy refusal of a
/// response row naming a different bank than the requested one (cursor unchanged).
pub fn pull_with_denial(
    cfg: &SyncClientConfig,
    banks: &[String],
    denial: Option<&dyn ErasureDenial>,
) -> Result<Vec<BankSyncStatus>, AmError> {
    let remote = required_remote(cfg)?;
    let mut out = Vec::new();
    for bank in banks {
        out.push(pull_bank(cfg, &remote, bank, denial)?);
    }
    Ok(out)
}

/// Bidirectional mode: pull first, then push (documented order).
pub fn bidirectional(
    cfg: &SyncClientConfig,
    banks: &[String],
) -> Result<Vec<BankSyncStatus>, AmError> {
    bidirectional_with_denial(cfg, banks, None)
}

/// Bidirectional mode under the erasure-denial authority: the pull half is
/// [`pull_with_denial`], the push half is unchanged.
///
/// # Errors
/// Transport, auth, or transient storage failures (cursor unchanged).
pub fn bidirectional_with_denial(
    cfg: &SyncClientConfig,
    banks: &[String],
    denial: Option<&dyn ErasureDenial>,
) -> Result<Vec<BankSyncStatus>, AmError> {
    let mut out = pull_with_denial(cfg, banks, denial)?;
    out.extend(push(cfg, banks)?);
    Ok(out)
}

/// Operator ack-skip for one permanently rejected mutation.
/// After the ack the `pull_cursor` may advance past that id.
///
/// # Errors
/// Storage failures.
pub fn ack_skip(cfg: &SyncClientConfig, sync_mutation_id: &str, at: &str) -> Result<bool, AmError> {
    cfg.sync.dead_letter_ack_skip(sync_mutation_id, at)
}

/// Masked status report (device id, cursors, pending counts, endpoint, auth
/// and encryption state, conflicts, dead letters). No secrets, no DEKs.
///
/// # Errors
/// Storage failures.
pub fn sync_status(cfg: &SyncClientConfig, banks: &[String]) -> Result<SyncStatusReport, AmError> {
    let mut rows = Vec::new();
    for bank in banks {
        let push_cursor = cfg.sync.cursor_get(bank, "push")?;
        let pull_cursor = cfg.sync.cursor_get(bank, "pull")?;
        let pending = pending_push(cfg, bank)?;
        let conflicts = cfg.sync.conflict_count(bank)?;
        let sample = cfg
            .sync
            .conflict_list(bank, 3)?
            .iter()
            .map(|c| format!("{} / {}", c.entity_id, c.loser_mutation_id))
            .collect();
        rows.push(BankSyncStatus {
            bank_id: bank.clone(),
            push_cursor,
            pull_cursor,
            pending_push: pending,
            dead_letter_open: cfg.sync.dead_letter_count(bank, DeadLetterStatus::Open)?,
            conflicts,
            conflict_sample: sample,
        });
    }
    Ok(SyncStatusReport {
        device_id: cfg.device_id.clone(),
        banks: rows,
        remote: cfg.remote.clone(),
        auth: if cfg.token.is_some() { "token" } else { "none" }.to_owned(),
        encryption: !cfg.cipher.is_empty(),
        encryption_summary: (!cfg.cipher.is_empty()).then_some(
            crate::protocol::EncryptionSummary {
                key_count: cfg.cipher.len(),
                store: None,
                current_fingerprint: None,
            },
        ),
        last_error: cfg.sync.cursor_get("__status", "last_error")?,
    })
}

/// Outbound mutation count for `bank`: each feed kind's rows past that kind's
/// own push watermark (item, assoc edge, triple, persona stable/preference,
/// belief, task, failure; the retired shared `push_wm` converts once).
/// `push_bank`, `pull_bank`, and `sync_status` report it, never a hard-coded
/// zero. Bounded by `PULL_PAGE_LIMIT` per kind and approximate: it never
/// subtracts rows a push would skip for a missing carrier.
pub(crate) fn pending_push(cfg: &SyncClientConfig, bank: &str) -> Result<i64, AmError> {
    let mut count = 0usize;
    for kind in crate::client_feed::PUSH_KINDS {
        let since = crate::client_feed::kind_watermark(cfg, bank, kind)?;
        count += crate::client_feed::feed_kind_count(cfg, bank, kind, &since)?;
    }
    Ok(i64::try_from(count).unwrap_or(i64::MAX))
}

fn required_remote(cfg: &SyncClientConfig) -> Result<String, AmError> {
    cfg.remote.clone().ok_or_else(|| {
        AmError::new(
            ErrorCode::InvalidArgument,
            "sync remote endpoint is required",
        )
    })
}

fn pull_bank(
    cfg: &SyncClientConfig,
    remote: &str,
    bank: &str,
    denial: Option<&dyn ErasureDenial>,
) -> Result<BankSyncStatus, AmError> {
    let mut cursor = cfg.sync.cursor_get(bank, "pull")?.unwrap_or_default();
    let engine = ApplyEngine {
        store: cfg.store.as_ref(),
        sync: cfg.sync.as_ref(),
        device_id: cfg.device_id.clone(),
        kms: None,
        policy: crate::freshness::FreshnessPolicy::default(),
    };
    loop {
        let request = PullRequest {
            bank: bank.to_owned(),
            cursor: cursor.clone(),
            limit: crate::PULL_PAGE_LIMIT,
        };
        let response: PullResponse = post_json(
            remote,
            "/sync/pull",
            cfg.token.as_deref(),
            &serde_json::to_value(&request).expect("serialize pull request"),
        )?;
        let mut frozen = false;
        for m in &response.mutations {
            // Refuse a wrong-bank row first, before serialization and
            // decryption: no journal write, no dead letter, no cursor advance.
            if m.bank != bank {
                let refused = format!(
                    "sync pull served mutation {} for bank {} while pulling bank {}",
                    m.sync_mutation_id, m.bank, bank
                );
                return Err(AmError::new(ErrorCode::Forbidden, refused));
            }
            let envelope = serde_json::to_string(m).expect("serialize mutation");
            let Some(m) = decrypt_for_apply(cfg, m, &envelope)? else {
                // Unusable row quarantined: freeze at the last durable
                // position; the operator must ack_skip.
                frozen = true;
                break;
            };
            let result = apply_mutation_guarded(&engine, denial, &m, &envelope)?;
            if matches!(result.outcome, ApplyOutcome::DeadLettered { .. }) {
                frozen = true;
                break;
            }
        }
        if frozen {
            break;
        }
        cursor.clone_from(&response.next_cursor);
        cfg.sync.cursor_set(bank, "pull", &cursor)?;
        if !response.has_more {
            break;
        }
    }
    Ok(BankSyncStatus {
        bank_id: bank.to_owned(),
        push_cursor: cfg.sync.cursor_get(bank, "push")?,
        pull_cursor: Some(cursor),
        pending_push: pending_push(cfg, bank)?,
        dead_letter_open: cfg.sync.dead_letter_count(bank, DeadLetterStatus::Open)?,
        conflicts: cfg.sync.conflict_count(bank)?,
        conflict_sample: Vec::new(),
    })
}

#[cfg(test)]
#[path = "client_bank_guard_tests.rs"]
mod client_bank_guard_tests;
#[cfg(test)]
#[path = "client_envelope_tests.rs"]
mod client_envelope_tests;
#[cfg(test)]
#[path = "client_plain_policy_tests.rs"]
mod client_plain_policy_tests;
#[cfg(test)]
#[path = "client_receipt_tests.rs"]
mod client_receipt_tests;
#[cfg(test)]
#[path = "client_resolve_fault_tests.rs"]
mod client_resolve_fault_tests;
#[cfg(test)]
#[path = "client_resolve_tests.rs"]
mod client_resolve_tests;
#[cfg(test)]
#[path = "client_rotation_tests.rs"]
mod client_rotation_tests;
#[cfg(test)]
#[path = "client_status_tests.rs"]
mod client_status_tests;
#[cfg(test)]
#[path = "client_tests.rs"]
pub(crate) mod client_tests;
#[cfg(test)]
#[path = "client_wire_blindness_tests.rs"]
mod client_wire_blindness_tests;
#[cfg(test)]
#[path = "client_wire_digest_tests.rs"]
mod client_wire_digest_tests;
