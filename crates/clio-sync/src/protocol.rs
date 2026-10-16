// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the sync wire protocol contract:
//!  the mutation envelope, payload batch, opaque cursor codec, the
//! default LWW conflict comparison, and the masked status report shape.
//!
//! The denial operation's own carrier and proof vocabulary lives in `denial`;
//! this module carries its per-mutation acknowledgement on `PushAck`.
//!
//! ## Owns
//! - `SyncMutation` / `SyncPayload` envelope and request/response types
//! - Opaque `sync_cursor` encode/decode (seq + checksum)
//! - Push-watermark token codec and compound body form
//! - `sync_lww_rule` comparison (`transaction_time` → admission score → device)
//! - Stable dead-letter reason codes and the masked `SyncStatusReport`
//!
//! ## Does not own
//! - Transport (HTTP server/client — `server` / `client` modules)
//! - Apply matrix, idempotent apply, DLQ policy (see `apply`)
//! - Client-side encryption primitives (see `crypto`)
//! - Admission scoring or gating (see `clio-admission`)
//!
//! ## Boundary
//!
//! Changes to the wire shape, cursor format, or conflict comparison should
//! normally require changes here. Changes to transport behavior should not
//! require changes here.
//!
//! Keep this module focused on the sync wire contract.

use base64::Engine;
use clio_types::{AmError, ErrorCode};
use serde::{Deserialize, Serialize};

use crate::crypto::SyncCipher;

/// Entity kind: typed memory item.
pub const ENTITY_KIND_ITEM: &str = "item";
/// Entity kind: bi-temporal SPO triple.
pub const ENTITY_KIND_TRIPLE: &str = "triple";
/// Entity kind: association edge (graph metadata).
pub const ENTITY_KIND_ASSOC: &str = "assoc_edge";
/// Entity kind: content-free lifecycle tombstone.
pub const ENTITY_KIND_TOMBSTONE: &str = "tombstone_meta";
/// Entity kind: persona domain record (stable entry or preference).
pub const ENTITY_KIND_PERSONA: &str = "persona";
/// Entity kind: belief object (create + append-only trajectory).
pub const ENTITY_KIND_BELIEF: &str = "belief";
/// Entity kind: task record (carrier item + versioned structured row).
pub const ENTITY_KIND_TASK: &str = "task";
/// Entity kind: failure record (carrier item + structured row).
pub const ENTITY_KIND_FAILURE: &str = "failure";

/// Operations carried by the sync envelope.
pub const OP_UPSERT: &str = "upsert";
/// Sync operation: close both time axes (bi-temporal invalidate).
pub const OP_INVALIDATE: &str = "invalidate";
/// Sync operation: logged operations removal.
pub const OP_DISCARD: &str = "discard";
/// Sync operation: reversible archive marker.
pub const OP_ARCHIVE: &str = "archive";
/// Sync operation: co-activation weight bump.
pub const OP_REINFORCE: &str = "reinforce";
/// Sync operation: association edge prune.
pub const OP_PRUNE: &str = "prune";

/// Dead-letter reason codes.
/// Dead-letter reason: schema or envelope malformed.
pub const REASON_MALFORMED: &str = "malformed";
/// Dead-letter reason: bank not authorized for this token.
pub const REASON_UNAUTHORIZED_BANK: &str = "unauthorized_bank";
/// Dead-letter reason: erased-subject content resurrection attempt.
pub const REASON_ERASED_SUBJECT: &str = "erased_subject";
/// Dead-letter reason: op not supported by this release.
pub const REASON_UNSUPPORTED_OP: &str = "unsupported_op";

/// Content as it travels: decrypted JSON, or client-encrypted opaque bytes.
///
/// Server-blind mode keeps the server unable to read content: the server
/// stores `ClientCipher` bytes verbatim without any DEK material.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", content = "data", rename_all = "snake_case")]
pub enum SyncContent {
    /// Plaintext logical content JSON (sealed at rest by each node's DEKs).
    Plain(serde_json::Value),
    /// Client-encrypted opaque content: the server never decrypts.
    Cipher(SyncCipher),
}

/// One durable mutation crossing the sync wire (minimum).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SyncMutation {
    /// Stable, globally unique idempotency key.
    pub sync_mutation_id: String,
    /// Bank isolation key.
    pub bank: String,
    /// Entity kind (`item` | `triple` | `assoc_edge` | `tombstone_meta` | ...).
    pub entity_kind: String,
    /// Stable business id of the mutated entity.
    pub entity_id: String,
    /// Operation (`upsert` | `invalidate` | `discard` | `archive` | `reinforce` | `prune`).
    pub op: String,
    /// Writer's transaction-time stamp (LWW key, ISO-8601 UTC text).
    pub transaction_time: String,
    /// Origin device id.
    pub device_id: String,
    /// Logical content, or client ciphertext when encryption is enabled.
    pub content: SyncContent,
    /// Hash of the plaintext logical content (even in ciphertext mode).
    pub content_hash: String,
    /// Optional origin admission receipt (`{admitted, score?, ...}`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admission_receipt: Option<serde_json::Value>,
}

impl SyncMutation {
    /// Validate the envelope shape (schema-level; never admission scoring).
    ///
    /// # Errors
    /// Missing ids, unknown kind/op, or empty transaction time.
    pub fn validate(&self) -> Result<(), AmError> {
        if self.sync_mutation_id.is_empty() || self.entity_id.is_empty() || self.bank.is_empty() {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "sync mutation requires sync_mutation_id, bank, and entity_id",
            ));
        }
        if self.transaction_time.is_empty() {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "sync mutation requires transaction_time",
            ));
        }
        match self.entity_kind.as_str() {
            ENTITY_KIND_ITEM
            | ENTITY_KIND_TRIPLE
            | ENTITY_KIND_ASSOC
            | ENTITY_KIND_TOMBSTONE
            | ENTITY_KIND_PERSONA
            | ENTITY_KIND_BELIEF
            | ENTITY_KIND_TASK
            | ENTITY_KIND_FAILURE => {}
            other => {
                return Err(AmError::new(
                    ErrorCode::InvalidArgument,
                    format!("unknown entity_kind: {other}"),
                ));
            }
        }
        match self.op.as_str() {
            OP_UPSERT | OP_INVALIDATE | OP_DISCARD | OP_ARCHIVE | OP_REINFORCE | OP_PRUNE => {}
            other => {
                return Err(AmError::new(
                    ErrorCode::InvalidArgument,
                    format!("unknown op: {other}"),
                ));
            }
        }
        Ok(())
    }

    /// Canonical plaintext content JSON used for hashing and apply.
    #[must_use]
    pub fn content_value(&self) -> &serde_json::Value {
        match &self.content {
            SyncContent::Plain(v) => v,
            SyncContent::Cipher(_) => {
                static EMPTY: std::sync::OnceLock<serde_json::Value> = std::sync::OnceLock::new();
                EMPTY.get_or_init(|| serde_json::json!({}))
            }
        }
    }
}

/// One push batch from a client.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SyncPayload {
    /// Origin device id.
    pub device_id: String,
    /// Bank this batch belongs to.
    pub bank: String,
    /// Mutations in order; replay-safe (idempotent apply by mutation id).
    pub mutations: Vec<SyncMutation>,
}

/// Server response to a push.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PushAck {
    /// New opaque push cursor (seq the server accepted through).
    pub cursor: String,
    /// Per-mutation outcomes, aligned with the payload order.
    pub accepted: Vec<String>,
    /// Mutation ids the server rejected outright (never silently dropped).
    pub rejected: Vec<String>,
    /// The durable proofs for the acknowledged denial operations, in payload
    /// order. Empty (and omitted from the wire) when the push acknowledged
    /// none, so a reader that does not know the field parses an ack unchanged.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub denial_acks: Vec<crate::denial::DenialAck>,
}

/// Pull request: fetch journal rows since `cursor`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PullRequest {
    /// Bank scope (server rejects unknown banks for this token).
    pub bank: String,
    /// Opaque cursor from a previous pull (empty = first pull).
    pub cursor: String,
    /// Max rows per page.
    pub limit: usize,
}

/// Pull response with one commit-ordered batch and the resume cursor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PullResponse {
    /// Journal rows in commit order.
    pub mutations: Vec<SyncMutation>,
    /// Opaque cursor to resume from (cursor after the last returned row).
    pub next_cursor: String,
    /// `true` when more rows remain in the bank.
    pub has_more: bool,
}

/// Decode an opaque cursor into its sequence number.
///
/// Cursors are opaque to clients; only the issuing peer decodes them.
///
/// # Errors
/// Malformed token or checksum mismatch.
pub fn decode_cursor(token: &str, bank: &str) -> Result<i64, AmError> {
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(token)
        .map_err(|_| AmError::new(ErrorCode::InvalidArgument, "malformed sync cursor"))?;
    let text = String::from_utf8(raw)
        .map_err(|_| AmError::new(ErrorCode::InvalidArgument, "malformed sync cursor"))?;
    let (seq, sum) = text
        .split_once(':')
        .ok_or_else(|| AmError::new(ErrorCode::InvalidArgument, "malformed sync cursor"))?;
    let seq: i64 = seq
        .parse()
        .map_err(|_| AmError::new(ErrorCode::InvalidArgument, "malformed sync cursor"))?;
    if checksum(bank, seq) != sum {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "sync cursor checksum mismatch",
        ));
    }
    Ok(seq)
}

/// Encode a sequence number as an opaque per-bank cursor token.
#[must_use]
pub fn encode_cursor(bank: &str, seq: i64) -> String {
    let text = format!("{seq}:{}", checksum(bank, seq));
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(text.as_bytes())
}

fn checksum(bank: &str, seq: i64) -> String {
    let digest = clio_store::sha256_hex("am-sync-cursor".as_bytes(), &format!("{bank}|{seq}"));
    digest[..12].to_owned()
}

/// Deterministic LWW comparison: later `transaction_time` wins;
/// ties compare origin admission scores; still-tied compares `device_id`.
#[must_use]
pub fn lww_compare(
    a_time: &str,
    a_score: Option<f64>,
    a_device: &str,
    b_time: &str,
    b_score: Option<f64>,
    b_device: &str,
) -> std::cmp::Ordering {
    match a_time.cmp(b_time) {
        std::cmp::Ordering::Equal => {}
        other => return other,
    }
    let a_score = a_score.unwrap_or(0.0);
    let b_score = b_score.unwrap_or(0.0);
    match a_score
        .partial_cmp(&b_score)
        .unwrap_or(std::cmp::Ordering::Equal)
    {
        std::cmp::Ordering::Equal => {}
        other => return other,
    }
    a_device.cmp(b_device)
}

/// Opaque watermark token codec for the client-side push change feed.
/// The watermark is a local timestamp, not a peer-issued cursor; clients own it.
pub fn encode_watermark(bank: &str, stamp: &str) -> String {
    let text = format!("wm:{stamp}:{}", checksum_str(bank, stamp));
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(text.as_bytes())
}

/// Decode an opaque watermark token into its timestamp.
///
/// # Errors
/// Malformed token or checksum mismatch.
pub fn decode_watermark(bank: &str, token: &str) -> Result<String, AmError> {
    if token.is_empty() {
        return Ok("1970-01-01T00:00:00Z".to_owned());
    }
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(token)
        .map_err(|_| AmError::new(ErrorCode::InvalidArgument, "malformed watermark"))?;
    let text = String::from_utf8(raw)
        .map_err(|_| AmError::new(ErrorCode::InvalidArgument, "malformed watermark"))?;
    let body = text
        .strip_prefix("wm:")
        .ok_or_else(|| AmError::new(ErrorCode::InvalidArgument, "malformed watermark"))?;
    let (stamp, sum) = body
        .rsplit_once(':')
        .ok_or_else(|| AmError::new(ErrorCode::InvalidArgument, "malformed watermark"))?;
    if !checksum_str_check(bank, stamp, sum) {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "watermark checksum mismatch",
        ));
    }
    Ok(stamp.to_owned())
}

fn checksum_str(bank: &str, stamp: &str) -> String {
    let digest = clio_store::sha256_hex("am-sync-wm".as_bytes(), &format!("{bank}|{stamp}"));
    digest[..12].to_owned()
}

fn checksum_str_check(bank: &str, stamp: &str, sum: &str) -> bool {
    !stamp.is_empty() && checksum_str(bank, stamp) == sum
}

/// Separator between the stamp and entity-id parts of a compound push
/// watermark body. It cannot appear inside a store stamp (ISO-8601 UTC text
/// or epoch-millis digits), so a canonical body splits unambiguously. The
/// store-side `sync_store_rows::WM_ENTITY_SEP` mirrors this for the feed SQL.
pub(crate) const WM_ENTITY_SEP: char = '\u{1f}';

/// Join a stamp and its per-kind tiebreak entity id into the canonical
/// compound watermark body handed to the change feeds.
pub(crate) fn join_watermark_body(stamp: &str, entity_id: &str) -> String {
    format!("{stamp}{WM_ENTITY_SEP}{entity_id}")
}

/// Canonicalize a decoded watermark body. A body that already carries the
/// separator is canonical; a body without one is the `(stamp, "")` pair,
/// re-encoded with the separator and an empty entity part. The decoded pair —
/// and with it the stamp-tie resume it implies — is preserved exactly.
pub(crate) fn canonical_watermark_body(body: &str) -> String {
    if body.contains(WM_ENTITY_SEP) {
        body.to_owned()
    } else {
        join_watermark_body(body, "")
    }
}

/// Masked sync status (device id, cursors, pending counts, endpoint, auth and
/// encryption state, last error, conflicts, dead letters). Never returns sync
/// auth secrets or DEK material: endpoint tokens are last-4 masked.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SyncStatusReport {
    /// This node's device id.
    pub device_id: String,
    /// Per-bank cursor snapshot.
    pub banks: Vec<BankSyncStatus>,
    /// Remote endpoint (URL only; token masked).
    pub remote: Option<String>,
    /// Auth state without secrets (`none` in dev mode, `token` otherwise).
    pub auth: String,
    /// Whether outbound content is client-encrypted.
    pub encryption: bool,
    /// Key-custody summary under the unchanged `encryption` flag, when any
    /// key is held. Additive: an older reader of this report ignores the
    /// field, and `encryption` keeps its type for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encryption_summary: Option<EncryptionSummary>,
    /// Last error text (PII-safe), when any.
    pub last_error: Option<String>,
}

/// Key-count and newest-key facts for a node that holds sync keys.
///
/// `store` and `current_fingerprint` are filled by the caller that assembled
/// the client from a store-backed key and are `None` when the keys came from
/// a supplied flag or environment value, whose provenance this crate does not
/// know. Fingerprints are derived and non-reversible; no field here ever
/// carries key bytes or a prefix of them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EncryptionSummary {
    /// How many keys this client holds, newest first in the client config.
    pub key_count: usize,
    /// The store the newest key is held in, when it came from a store.
    pub store: Option<String>,
    /// The newest key's non-reversible fingerprint, when known.
    pub current_fingerprint: Option<String>,
}

/// Per-bank sync status row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BankSyncStatus {
    /// Bank id.
    pub bank_id: String,
    /// Opaque push cursor (server-accepted through this seq).
    pub push_cursor: Option<String>,
    /// Opaque pull cursor (locally applied through this seq).
    pub pull_cursor: Option<String>,
    /// Outgoing mutations not yet pushed.
    pub pending_push: i64,
    /// Open dead-letter count (blocks `pull_cursor` advance).
    pub dead_letter_open: i64,
    /// Recorded LWW conflict count.
    pub conflicts: i64,
    /// Sample of recent conflict summaries (ids only).
    pub conflict_sample: Vec<String>,
}

/// Map a reject into a stable DLQ reason code.
#[must_use]
pub fn reason_code_for(err: &AmError) -> &'static str {
    match err.code {
        ErrorCode::ErasedSubject => REASON_ERASED_SUBJECT,
        ErrorCode::Forbidden => REASON_UNAUTHORIZED_BANK,
        ErrorCode::NotImplemented => REASON_UNSUPPORTED_OP,
        _ => REASON_MALFORMED,
    }
}

#[cfg(test)]
#[path = "protocol_tests.rs"]
mod protocol_tests;
