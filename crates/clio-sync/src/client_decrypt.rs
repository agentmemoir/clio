// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for opening one pulled mutation before apply:
//! decrypting its sealed content with a held key, refusing (quarantining) a row
//! this node cannot trust, resolving its identifier handle, and restoring the
//! origin admission receipt the wire loses.
//!
//! ## Owns
//! - `decrypt_for_apply`: the pull-side open and refusal path
//! - `restore_admission_receipt`: the opened payload's receipt, back on the wire row
//!
//! ## Does not own
//! - The pull loop and its cursor policy (see `client`)
//! - The apply matrix and its outcomes (see `apply`)
//! - Key material and the cipher shape (see `crypto`)
//!
//! ## Boundary
//!
//! Changes to how a pulled row is opened or refused should normally require
//! changes here. Changes to the cursor or apply order should not.
//!
//! Keep this module focused on opening a pulled row before apply.

use crate::apply::{ApplyEngine, admission_score, quarantine};
use crate::client::SyncClientConfig;
use crate::client_refusal::{PLAINTEXT_REFUSED_REASON, missing_key_reason, open_failed_reason};
use crate::protocol::{SyncContent, SyncMutation};
use clio_store::DeadLetterStatus;
use clio_types::AmError;
use serde_json::Value;

/// Decrypt a pulled cipher mutation with the local sync keys before apply,
/// restore its origin admission receipt, and resolve its identifier handle back
/// to the real identifier from the opened payload. Returns `None` when the row
/// was quarantined (unusable content, plaintext content, or an identifier that
/// cannot be resolved). Rows the operator already ack-skipped pass through
/// unchanged so apply treats them as duplicates and the cursor may advance.
pub(crate) fn decrypt_for_apply(
    cfg: &SyncClientConfig,
    m: &SyncMutation,
    envelope_json: &str,
) -> Result<Option<SyncMutation>, AmError> {
    if cfg
        .sync
        .dead_letter_status(&m.sync_mutation_id)?
        .is_some_and(|status| status == DeadLetterStatus::AckedSkip)
    {
        return Ok(Some(m.clone()));
    }
    let engine = ApplyEngine {
        store: cfg.store.as_ref(),
        sync: cfg.sync.as_ref(),
        device_id: cfg.device_id.clone(),
        kms: None,
        clock: &crate::clock::now_epoch,
        max_envelope_skew_secs: cfg.max_envelope_skew_secs,
    };
    // One refusal shape for every row this node cannot trust: quarantine it
    // under the row's own bank and stop, so the cursor freezes at the prior id
    // and the operator can discard the row deliberately with `ack_skip`.
    let refuse = |bank: &str, reason: &str| -> Result<Option<SyncMutation>, AmError> {
        quarantine(
            &engine,
            bank,
            m,
            envelope_json,
            crate::protocol::REASON_MALFORMED,
            reason,
        )?;
        Ok(None)
    };
    let SyncContent::Cipher(cipher) = &m.content else {
        // A plaintext row carries no authenticated envelope, so a relay can
        // fabricate it or downgrade a sealed row to it freely. Without a key
        // this node cannot tell either apart, so it refuses the row too.
        let reason = if cfg.cipher.is_empty() {
            crate::client_refusal::PLAINTEXT_REFUSED_KEYLESS_REASON
        } else {
            PLAINTEXT_REFUSED_REASON
        };
        return refuse(&m.bank, reason);
    };
    if cfg.cipher.is_empty() {
        return refuse(&m.bank, "no sync cipher key configured");
    }
    let held: Vec<String> = cfg
        .cipher
        .iter()
        .map(crate::crypto::SyncCipherKey::key_id)
        .collect();
    // A row with no key identifier no longer opens: current writes always
    // stamp one, and trial-opening every held key widens the wrong-key
    // surface. The refusal is a stable quarantine the operator discards.
    let Some(stamped) = &cipher.key_id else {
        return refuse(
            &m.bank,
            "sync cipher carries no key identifier; this release opens only ciphertext stamped \
             with a held key identifier. Discard the row deliberately with the `sync_ack_skip` MCP \
             tool, passing its mutation id with confirm",
        );
    };
    // A row stamped with an identifier none of the held keys produce was written
    // by a key this node does not hold. Refusing before any open attempt makes
    // the reason name the missing key, not a generic open failure; both values
    // are derived and non-reversible, and neither is ever a key or a prefix of
    // one.
    let Some(key) = cfg.cipher.iter().find(|k| k.key_id() == *stamped) else {
        return refuse(&m.bank, &missing_key_reason(stamped, &held));
    };
    // A ciphertext the stamped key cannot open (wrong key, tampering,
    // corruption) is a permanent per-row failure: quarantine it so
    // `pull_cursor` freezes at the prior id and the operator can `ack_skip`.
    let Ok(bytes) = key.open(cipher, &m.execution_view().aad()) else {
        return refuse(&m.bank, &open_failed_reason(stamped));
    };
    let Ok(plain) = serde_json::from_slice::<Value>(&bytes) else {
        return refuse(&m.bank, "decrypted sync content was not JSON");
    };
    let mut opened = m.clone();
    opened.content = SyncContent::Plain(plain.clone());
    // The opaque identifier handle, if this row has one, is resolved here:
    // the ciphertext is open, the real identifier is inside the payload, and
    // `apply_mutation` has not run yet in this same loop iteration. A handled
    // row whose payload carries no identifier is refused rather than applied
    // with the handle still in place.
    let kind = opened.entity_kind.clone();
    let bank = opened.bank.clone();
    if let Err(failure) = crate::handle_resolution::resolve_entity_id(&mut opened, &plain) {
        return refuse(&bank, &failure.reason(&kind, &bank));
    }
    restore_admission_receipt(&mut opened, &plain);
    Ok(Some(opened))
}

/// Lift the origin admission receipt out of an opened sealed payload and put it
/// back on the mutation before apply, so the conflict comparison sees the same
/// receipt a plaintext row carries on the wire. A payload with no usable
/// receipt leaves the wire value, which is the documented absent-score fallback
/// (`lww_compare` maps `None` to `0.0`) and not an error.
fn restore_admission_receipt(opened: &mut SyncMutation, plain: &Value) {
    let Some(receipt) = plain.get(crate::client_feed::package::ADMISSION_RECEIPT_KEY) else {
        return;
    };
    if admission_score(receipt).is_some() {
        opened.admission_receipt = Some(receipt.clone());
    }
}

#[cfg(test)]
#[path = "client_decrypt_tests.rs"]
mod client_decrypt_tests;
