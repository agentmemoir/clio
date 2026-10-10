// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the network-credential token store: the
//! record type, its validation, digest generation, and the fail-closed
//! load/save of one owner-only file under the config directory.
//!
//! ## Owns
//! - The `AuthTokenRecord` shape (`label`, `secret_sha256`, `actor`, `banks`,
//!   `created_at`) and its serialization
//! - Label, actor, and bank validation; duplicate-label and unknown-label rules
//! - Secret generation (32 CSPRNG bytes, base64url) and the SHA-256 digest rule
//! - The owner-only permission check on load and the owner-only atomic save
//! - The store-file path resolution (override env, then the XDG default)
//!
//! ## Does not own
//! - The writer lock itself (see `auth_tokens_lock`)
//! - Any runtime reader: no transport loads the store in this phase
//! - The deployment overlay or its sealing (see `config::persist`)
//! - The CLI rendering of these records (see the `clio` auth surface)
//!
//! ## Boundary
//!
//! Changes to the record shape, validation, or fail-closed loading should
//! normally require changes here. Changes to transport behavior or CLI output
//! should not require changes here.
//!
//! Keep this module focused on the credential record and its store file.
//!
//! # Fail-closed reads
//!
//! A missing file is an empty store for the management verbs; everything else
//! that is not a valid store is refused: a file another user could read, an
//! empty or truncated file, malformed JSON, an unknown field, an invalid
//! record, or a repeated label. A refusal names the path and the reason and
//! never persists anything.
//!
//! # Concurrent writers
//!
//! Every mutation takes the exclusive hold on `<store>.lock` for its whole
//! read-modify-write, so two `clio auth token add` runs cannot both read an
//! empty store and each write its own record over the other's. A writer that
//! cannot take the hold within a bounded wait refuses without printing a
//! secret. The hold follows the item store's rule (`crates/clio-store/src/
//! store_lock.rs`): advisory, bounded, refusing rather than waiting forever.

use crate::context::Actor;
use crate::error::{AmError, ErrorCode};
use base64::Engine;
use clio_types::civil_from_days;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub use crate::auth_tokens_lock::{ExclusiveHold, lock_file_path};
// The validators live in `auth_tokens_validation` because both the write path
// and the load path call them, and a store whose two halves disagree about
// what a valid record is is the defect that module exists to prevent.
pub use crate::auth_tokens_validation::{
    ct_eq, valid_created_at, validate_banks, validate_label, validate_loaded,
};

/// Env var naming the token store file (an explicit path override).
pub const AUTH_TOKEN_FILE_ENV: &str = "CLIO_AUTH_TOKEN_FILE";

/// Default token store file name under the XDG config app directory.
pub const AUTH_TOKEN_FILE_NAME: &str = "auth_tokens.json";

/// One network credential: who it names and what it may reach.
///
/// The secret itself is never a field: `secret_sha256` is the only
/// secret-derived value that exists on disk, and the plaintext lives in
/// memory just long enough to be printed once by the `add` verb.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthTokenRecord {
    pub label: String,
    pub secret_sha256: String,
    pub actor: Actor,
    pub banks: Vec<String>,
    pub created_at: String,
}

/// Resolve the token store path: `CLIO_AUTH_TOKEN_FILE` when set, else the XDG
/// default `<config>/clio/auth_tokens.json`. A blank override counts as unset,
/// matching every other env override in this crate. `None` means no config
/// directory is known; callers refuse rather than guess a location.
///
/// The lookup is a parameter rather than a direct `std::env::var` so a test
/// never reads the host environment, and so the production caller and the
/// tests drive the same code instead of two copies of it.
#[must_use]
pub fn resolve_token_store_path_with(
    explicit: Option<PathBuf>,
    get: impl Fn(&str) -> Option<String>,
) -> Option<PathBuf> {
    if let Some(p) = explicit {
        return Some(p);
    }
    if let Some(raw) = get(AUTH_TOKEN_FILE_ENV) {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed));
        }
        // A blank override is unset, not a path; fall through to the default.
    }
    let dir = crate::config::default_config_dir(&get)?;
    Some(dir.join(AUTH_TOKEN_FILE_NAME))
}

/// Load the store. A missing file is an empty store; every other unreadable
/// state is refused (see the module docs).
///
/// # Errors
/// [`ErrorCode::Forbidden`] when the file is not owner-only,
/// [`ErrorCode::ConfigCorrupt`] when it cannot be read, parsed, or validated.
pub fn load_auth_tokens(path: &Path) -> Result<Vec<AuthTokenRecord>, AmError> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    owner_only_refusal(path)?;
    let text = std::fs::read_to_string(path).map_err(|e| {
        AmError::new(
            ErrorCode::ConfigCorrupt,
            format!("failed to read token store {}: {e}", path.display()),
        )
    })?;
    let records: Vec<AuthTokenRecord> = serde_json::from_str(&text).map_err(|e| {
        AmError::new(
            ErrorCode::ConfigCorrupt,
            format!("token store {}: malformed or empty: {e}", path.display()),
        )
    })?;
    validate_loaded(&records)?;
    Ok(records)
}

/// Save the store: owner-only, atomic (temp file + rename), full replacement.
///
/// The parent directory is created when missing and restricted to the owner
/// only when this call created it. A directory the operator already had keeps
/// its mode: tightening a directory the store did not create changes what
/// every other file in it can be read by, which is a side effect the
/// deployment-config writer (`config::persist::save_overlay_file`) does not
/// have either. The file's own `0600` mode is what the load-time refusal
/// checks, and it is set by [`crate::config::write_private_atomic`].
///
/// # Errors
/// I/O failures creating the directory or writing the file.
///
/// # Panics
/// Serialization of this record shape cannot fail; a failure is a bug and
/// panics rather than writing a partial store.
pub fn save_auth_tokens(path: &Path, records: &[AuthTokenRecord]) -> Result<(), AmError> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        let existed = parent.exists();
        std::fs::create_dir_all(parent).map_err(|e| {
            AmError::new(
                ErrorCode::Internal,
                format!("failed to create {}: {e}", parent.display()),
            )
        })?;
        if !existed {
            crate::config::restrict_dir(parent);
        }
    }
    let text = serde_json::to_string_pretty(records).expect("serialize auth token records");
    crate::config::write_private_atomic(path, &text).map_err(|e| {
        AmError::new(
            ErrorCode::Internal,
            format!("failed to write token store {}: {e}", path.display()),
        )
    })
}

/// Build a new record and its one-time secret: validate the input, generate
/// 32 CSPRNG bytes, and store only their SHA-256 digest. The caller prints the
/// secret once and then drops it.
///
/// # Errors
/// [`ErrorCode::InvalidArgument`] for a label, actor, or bank set that fails
/// [`validate_label`] / `Actor::parse` / [`validate_banks`].
pub fn new_auth_token(
    label: &str,
    actor_raw: &str,
    banks_raw: &[String],
) -> Result<(AuthTokenRecord, String), AmError> {
    validate_label(label)?;
    let actor = Actor::parse(actor_raw)?;
    let banks = validate_banks(banks_raw)?;
    let secret = generate_secret();
    Ok((
        AuthTokenRecord {
            label: label.to_owned(),
            secret_sha256: digest_hex(&secret),
            actor,
            banks,
            created_at: now_utc(),
        },
        secret,
    ))
}

/// Add one token under the store's exclusive hold, so the record that lands on
/// disk is the one whose secret the caller goes on to print.
///
/// The secret is generated inside the hold and returned only after the save is
/// confirmed, so a token that could not be stored never prints a secret nobody
/// can use. A caller that does not need the whole read-modify-write under one
/// hold can compose [`new_auth_token`], [`add_record`], and [`save_auth_tokens`]
/// itself; this is the composition the CLI uses, made atomic.
///
/// # Errors
/// [`ErrorCode::InvalidArgument`] for a bad label/actor/bank set or a taken
/// label, [`ErrorCode::Internal`] or [`ErrorCode::ConfigCorrupt`] for the
/// store itself.
pub fn add_token(
    path: &Path,
    label: &str,
    actor_raw: &str,
    banks_raw: &[String],
) -> Result<(AuthTokenRecord, String), AmError> {
    // The input is validated before the hold: a refusal for bad input should
    // not make the caller wait behind another writer, and it changes nothing.
    validate_label(label)?;
    Actor::parse(actor_raw)?;
    validate_banks(banks_raw)?;
    let _hold = exclusive_hold(path)?;
    let records = load_auth_tokens(path)?;
    let (record, secret) = new_auth_token(label, actor_raw, banks_raw)?;
    let records = add_record(records, record.clone())?;
    save_auth_tokens(path, &records)?;
    Ok((record, secret))
}

/// Revoke the token named by `label` under the store's exclusive hold, so a
/// concurrent `add` is not lost by a rewrite that read before it arrived.
///
/// # Errors
/// [`ErrorCode::NotFound`] when no record carries that label, plus every
/// failure [`add_token`] can have.
pub fn revoke_token(path: &Path, label: &str) -> Result<(), AmError> {
    let _hold = exclusive_hold(path)?;
    let records = remove_record(load_auth_tokens(path)?, label)?;
    save_auth_tokens(path, &records)
}

/// Take the exclusive hold, creating the store's directory first.
///
/// The lock file lives beside the store, so a first `add` into a directory that
/// does not exist yet would have nothing to open. The directory is created
/// here — owner-only, because this call created it — rather than inside
/// [`save_auth_tokens`], which the hold must already be taken by the time it
/// runs.
fn exclusive_hold(path: &Path) -> Result<ExclusiveHold, AmError> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty())
        && !parent.exists()
    {
        std::fs::create_dir_all(parent).map_err(|e| {
            AmError::new(
                ErrorCode::Internal,
                format!("failed to create {}: {e}", parent.display()),
            )
        })?;
        crate::config::restrict_dir(parent);
    }
    ExclusiveHold::acquire(path)
}

/// Insert `record` into `records`, refusing a label that already exists.
///
/// # Errors
/// [`ErrorCode::InvalidArgument`] when the label is taken.
pub fn add_record(
    records: Vec<AuthTokenRecord>,
    record: AuthTokenRecord,
) -> Result<Vec<AuthTokenRecord>, AmError> {
    if records.iter().any(|r| r.label == record.label) {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            format!("a token labelled `{}` already exists", record.label),
        ));
    }
    let mut records = records;
    records.push(record);
    Ok(records)
}

/// Remove the record named by `label`.
///
/// # Errors
/// [`ErrorCode::NotFound`] when no record carries that label.
pub fn remove_record(
    records: Vec<AuthTokenRecord>,
    label: &str,
) -> Result<Vec<AuthTokenRecord>, AmError> {
    let before = records.len();
    let kept: Vec<AuthTokenRecord> = records.into_iter().filter(|r| r.label != label).collect();
    if kept.len() == before {
        return Err(AmError::new(
            ErrorCode::NotFound,
            format!("no token labelled `{label}`"),
        ));
    }
    Ok(kept)
}

/// Generate one secret: 32 random bytes, base64url without padding (43
/// characters, URL-safe, no `=`).
#[must_use]
pub fn generate_secret() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// The SHA-256 digest of a secret, lowercase hex — the only secret-derived
/// value that is ever persisted. Hex writing into a `String` cannot fail, so
/// the write result is discarded (the `backup_path` hash helper's pattern).
#[must_use]
pub fn digest_hex(secret: &str) -> String {
    use std::fmt::Write as _;
    let digest = Sha256::digest(secret.as_bytes());
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in &digest {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Refuse a store file whose mode grants anything to group or other.
///
/// # Errors
/// [`ErrorCode::Forbidden`] naming the path; never the host's mode value.
#[cfg(unix)]
fn owner_only_refusal(path: &Path) -> Result<(), AmError> {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(path)
        .map_err(|e| {
            AmError::new(
                ErrorCode::ConfigCorrupt,
                format!("failed to read token store {}: {e}", path.display()),
            )
        })?
        .permissions()
        .mode();
    if mode.trailing_zeros() >= 6 {
        return Ok(());
    }
    Err(AmError::new(
        ErrorCode::Forbidden,
        format!(
            "token store {}: refused because it is not owner-only; \
             restrict it to the owner (0600) and run again",
            path.display()
        ),
    ))
}

/// Non-Unix platforms have no mode bits to check; the owner-only rule is a
/// no-op there, matching `persist::restrict_dir`.
#[cfg(not(unix))]
fn owner_only_refusal(_path: &Path) -> Result<(), AmError> {
    Ok(())
}

/// UTC `YYYY-MM-DDThh:mm:ssZ` for a Unix second count. Split from [`now_utc`]
/// so a test can assert the stamp at a known instant rather than only at
/// whatever second the test runs.
#[must_use]
pub fn utc_stamp(seconds: u64) -> String {
    let days = i64::try_from(seconds / 86_400).unwrap_or(0);
    let (y, m, d) = civil_from_days(days);
    let rem = seconds % 86_400;
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Current UTC time, RFC 3339 at seconds precision.
#[must_use]
pub fn now_utc() -> String {
    utc_stamp(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs()),
    )
}

#[cfg(test)]
#[path = "auth_tokens_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "auth_tokens_integrity_tests.rs"]
mod integrity_tests;
