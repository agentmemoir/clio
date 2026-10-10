// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the MCP-bound multi-host sync tools
//! `sync_push` / `sync_pull` (client modes incl. bidirectional)
//! `sync_status` (masked local report), `sync_key_status` (masked key
//! management report), `sync_serve` (background server with a removed-token
//! rejection, dev-mode refusal, and TTL), and `sync_ack_skip` (confirmed
//! dead-letter ack).
//!
//! ## Owns
//! - JSON argument decoding and result shaping for the six sync tools
//! - Client config assembly (`device_id`, store/sync handles, remote, token,
//!   and encryption keys): the transfer path reads the explicit key file when
//!   `CLIO_SYNC_KEY_FILE` names one, else the recorded store, while the status
//!   path takes a `sync_key` override and otherwise reads the recorded store,
//!   marker-authoritative, so the two paths can read different custody
//! - Token masking on every response tree (last-4 at most, never the clear)
//! - Dev-mode refusal mirroring on `sync_serve` and dry-run gating on
//!   `sync_ack_skip`
//!
//! ## Does not own
//! - Sync protocol semantics, apply matrix, cursors (see `clio-sync`)
//! - Server transport internals (`clio-sync::server`)
//! - Store binding / DEKs (`McpState` wiring in `runtime`)
//!
//! ## Boundary
//!
//! Changes to MCP argument decoding or response shaping should normally
//! require changes here. Changes to sync behavior should not require changes
//! here.
//!
//! Keep this module focused on MCP sync dispatch.

use crate::runtime::{McpState, now_utc};
use crate::write_tools::{opt_bool, opt_str, req_str};
use clio_sync::client::SyncModes;
use clio_sync::prelude::{
    SyncCipherKey, SyncClientConfig, SyncServerOptions, SyncStatusReport, ack_skip, bidirectional,
    pull, push, sync_serve, sync_status,
};
use clio_types::{AmError, ErrorCode};
use serde_json::{Value, json};
use std::path::Path;

/// Route a sync tool name to its in-process handler (raw result).
#[inline(never)]
pub(crate) fn dispatch_inner(state: &McpState, name: &str, args: &Value) -> Result<Value, AmError> {
    match name {
        "sync_push" => sync_transfer(state, args, "push"),
        "sync_pull" => sync_transfer(state, args, "pull"),
        "sync_status" => sync_status_tool(state, args),
        "sync_key_status" => sync_key_status_tool(state, args),
        "sync_serve" => sync_serve_tool(state, args),
        "sync_ack_skip" => sync_ack_skip_tool(state, args),
        other => Err(AmError::not_implemented(other)),
    }
}

/// `sync_push` / `sync_pull(remote, mode?, banks?, token?)`.
///
/// The tool name fixes the default mode; an explicit `mode` argument overrides
/// it (`bidirectional` pulls then pushes in one operator action). The remote
/// bearer token is echoed masked (last 4 at most), never in the clear.
#[inline(never)]
fn sync_transfer(state: &McpState, args: &Value, default_mode: &str) -> Result<Value, AmError> {
    sync_transfer_with(state, args, default_mode, &|key| std::env::var(key).ok())
}

/// The transfer run for one environment lookup, so a test can pin the custody
/// keys without reading the machine's own key store.
pub(crate) fn sync_transfer_with(
    state: &McpState,
    args: &Value,
    default_mode: &str,
    get: &dyn Fn(&str) -> Option<String>,
) -> Result<Value, AmError> {
    // The transfer no longer takes a `sync_key`: the key is read from this
    // node's recorded store. Reject any supplied value loudly rather than
    // silently ignore it, because a dropped key would leave the caller
    // believing content was sealed while it left in the clear.
    if args.get("sync_key").is_some() {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "`sync_push`/`sync_pull` no longer accept `sync_key`; the key is read from this node's recorded sync key store (run `clio sync keygen`)",
        ));
    }
    let mut cfg = client_cfg(state, args);
    cfg.cipher = custody_cipher(get)?;
    let banks = banks_of(state, args)?;
    let mode = SyncModes::parse(opt_str(args, "mode").unwrap_or(default_mode))?;
    let rows = match mode {
        SyncModes::Push => push(&cfg, &banks)?,
        SyncModes::Pull => pull(&cfg, &banks)?,
        SyncModes::Bidirectional => bidirectional(&cfg, &banks)?,
    };
    Ok(json!({
        "mode": opt_str(args, "mode").unwrap_or(default_mode),
        "remote": cfg.remote,
        "token": mask_token(cfg.token.as_ref()),
        "encryption": !cfg.cipher.is_empty(),
        "banks": rows,
        "timestamp": now_utc(),
    }))
}

/// `sync_status()`: masked local report (device id, cursors, pending,
/// conflicts, DLQ, auth/encryption state). The report shape carries no
/// secrets by construction; serialization is the response.
#[inline(never)]
fn sync_status_tool(state: &McpState, args: &Value) -> Result<Value, AmError> {
    sync_status_with(state, args, &|key| std::env::var(key).ok())
}

/// The status report for one environment lookup, so a test can pin the
/// custody summary without reading the machine's own key store.
pub(crate) fn sync_status_with(
    state: &McpState,
    args: &Value,
    get: &dyn Fn(&str) -> Option<String>,
) -> Result<Value, AmError> {
    let mut cfg = client_cfg(state, args);
    cfg.cipher = cipher_keys(args)?;
    // With no keys supplied, the recorded custody store fills the client
    // config: the store marker decides which store answers. The transfer path
    // is not this read: it takes an explicit `CLIO_SYNC_KEY_FILE` first, so
    // the two surfaces agree only when no explicit file is named. That
    // divergence is accepted and pinned by the key tests.
    apply_custody_fallback(&mut cfg, args, get)?;
    let banks = banks_of(state, args)?;
    let mut report = sync_status(&cfg, &banks)?;
    fill_encryption_summary(&mut report, get);
    Ok(serde_json::to_value(report).expect("serialize status report"))
}

/// Fill the status client config from the recorded custody store when the
/// caller supplied no keys. The store marker decides which store answers; this
/// read never consults the explicit `CLIO_SYNC_KEY_FILE`, unlike the transfer
/// path, which takes that file first. When the variable names a file over a
/// marker that records the other store, the two surfaces read different
/// custody; that divergence is accepted. A store that cannot be reached is an
/// error here: encryption must never silently turn off between two surfaces
/// that both report it.
fn apply_custody_fallback(
    cfg: &mut SyncClientConfig,
    args: &Value,
    get: &dyn Fn(&str) -> Option<String>,
) -> Result<(), AmError> {
    if args.get("sync_key").is_none_or(serde_json::Value::is_null) && cfg.cipher.is_empty() {
        let keys = clio_config::sync_key::load_all_base64(&|key: &str| get(key))?;
        cfg.cipher = keys
            .iter()
            .map(|secret| SyncCipherKey::from_secret(secret))
            .collect();
    }
    Ok(())
}

/// Fill the store-backed custody facts the sync layer cannot know: which store
/// holds the newest key and its fingerprint. Keys supplied through the tool
/// argument have no custody record of their own, so the fields stay `None`
/// unless the machine's own key store answers. A custody lookup that fails
/// leaves the fields `None` too: the summary is additive and must never turn a
/// working status call into a failure.
fn fill_encryption_summary(report: &mut SyncStatusReport, get: &dyn Fn(&str) -> Option<String>) {
    let Some(summary) = report.encryption_summary.as_mut() else {
        return;
    };
    let custody = clio_config::sync_key::report(&|key: &str| get(key)).ok();
    if let Some(custody) = custody.filter(|custody| custody.fingerprint.is_some()) {
        summary.store = Some(custody.store.as_str().to_owned());
        summary.current_fingerprint = custody.fingerprint;
    }
}

/// `sync_key_status()`: masked local key-management report (store, fingerprint,
/// creation time, key count). Never the key and never a prefix of it.
#[inline(never)]
fn sync_key_status_tool(_state: &McpState, _args: &Value) -> Result<Value, AmError> {
    sync_key_status_with(&|key| std::env::var(key).ok())
}

/// The key-status report for one environment lookup, so a test can pin the
/// shape without reading the machine's own key store.
pub(crate) fn sync_key_status_with(get: &dyn Fn(&str) -> Option<String>) -> Result<Value, AmError> {
    let report = clio_config::sync_key::report(&|key: &str| get(key))?;
    Ok(json!({
        "store": report.store.as_str(),
        "fingerprint": report.fingerprint,
        "created_at": report.created_at,
        "count": report.count,
        "timestamp": now_utc(),
        "keys": keys_payload(&report.keys),
    }))
}

/// The per-key facts the key report shows: fingerprint, creation time, whether
/// this entry is the current key, and where it is held. No entry ever carries
/// key bytes or a prefix of them.
fn keys_payload(keys: &[clio_config::sync_key::SyncKeyEntry]) -> Vec<Value> {
    keys.iter()
        .map(|entry| {
            json!({
                "fingerprint": entry.fingerprint,
                "created_at": entry.created_at,
                "current": entry.current,
                "store": entry.store,
            })
        })
        .collect()
}

/// `sync_serve(bind?, banks?, token_file?, ttl_secs?, dev?)`: start the sync
/// server on a background thread. A caller that still supplies `token` is
/// refused: the single-token server input was removed, and the store file is
/// the one credential source. Refuses an anonymous bind outside explicit dev
/// mode (mirroring the clio-sync server's own refusal, defense in depth). The
/// response carries the resolved address (ephemeral port included), the
/// effective TTL, and the served banks.
#[inline(never)]
fn sync_serve_tool(state: &McpState, args: &Value) -> Result<Value, AmError> {
    // Reject a supplied `token` loudly rather than silently ignore it: the
    // caller would otherwise believe the bind is authenticated while a
    // token-only request can never resolve to a scoped record.
    if args.get("token").is_some() {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "`sync_serve` no longer accepts `token`; add a scoped record with \
             `clio auth token add` and pass its store file as `token_file`",
        ));
    }
    let dev = opt_bool(args, "dev", false)?;
    let token_file = opt_str(args, "token_file").map(std::path::PathBuf::from);
    if token_file.is_none() && !dev {
        return Err(AmError::new(
            ErrorCode::Forbidden,
            "sync_serve requires auth material outside dev mode (refusing anonymous bind)",
        ));
    }
    let ttl_secs = args.get("ttl_secs").and_then(Value::as_u64);
    let banks = banks_of(state, args)?;
    let server = sync_serve(SyncServerOptions {
        bind: opt_str(args, "bind").unwrap_or("127.0.0.1:0").to_owned(),
        store: state.store.clone(),
        sync: state.sync.clone(),
        kms: state.kms.clone(),
        banks: banks.clone(),
        token_file,
        dev_mode: dev,
        ttl_secs,
        deadline_secs: None,
        device_id: state.device_id.clone(),
    })?;
    Ok(json!({
        "addr": server.addr(),
        "ttl_secs": ttl_secs.unwrap_or(clio_sync::SERVE_TTL_DEFAULT_SECS),
        "banks": banks,
        "dev": dev,
        "timestamp": now_utc(),
    }))
}

/// `sync_ack_skip(sync_mutation_id, confirm?, reason?)`: operator ack-skip of
/// one permanently rejected mutation. Without `confirm=true` the call is a
/// dry-run shape (`{acked: false, would_ack: id}`); with it, the dead-letter
/// is acknowledged and the pull cursor may advance past the id.
#[inline(never)]
fn sync_ack_skip_tool(state: &McpState, args: &Value) -> Result<Value, AmError> {
    let id = req_str(args, "sync_mutation_id")?.to_owned();
    let confirm = opt_bool(args, "confirm", false)?;
    if !confirm {
        return Ok(json!({ "acked": false, "would_ack": id }));
    }
    let mut cfg = client_cfg(state, args);
    cfg.cipher = cipher_keys(args)?;
    let acked = ack_skip(&cfg, &id, &state.resolve_timestamp(args))?;
    Ok(json!({
        "acked": acked,
        "sync_mutation_id": id,
        "reason": opt_str(args, "reason"),
        "timestamp": now_utc(),
    }))
}

/// Build the clio-sync client config from MCP state and args, with no
/// encryption keys: each caller sets `cipher` from its own source.
fn client_cfg(state: &McpState, args: &Value) -> SyncClientConfig {
    SyncClientConfig {
        device_id: state.device_id.clone(),
        store: state.store.clone(),
        sync: state.sync.clone(),
        remote: opt_str(args, "remote").map(str::to_owned),
        token: opt_str(args, "token").map(str::to_owned),
        cipher: Vec::new(),
    }
}

/// The transfer path's key secrets, newest first.
///
/// When `CLIO_SYNC_KEY_FILE` names a file it is the newest key and outranks a
/// recorded store marker; the archived keys are appended after it. When the
/// variable is unset the recorded store answers, with its own archived keys.
/// A caller cannot supply a key on this path; the argument is rejected first.
///
/// # Errors
/// The explicit file is missing, not owner-only, unreadable, or the wrong
/// length; or the recorded store is unreachable; or the archive cannot be read.
fn custody_secrets(get: &dyn Fn(&str) -> Option<String>) -> Result<Vec<String>, AmError> {
    let explicit = get(clio_config::SYNC_KEY_FILE_ENV)
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    if let Some(path) = explicit {
        let mut keys = vec![clio_config::sync_key::read_key_file_base64(Path::new(
            &path,
        ))?];
        keys.extend(clio_config::sync_key::load_archive_base64(&|key: &str| {
            get(key)
        })?);
        return Ok(keys);
    }
    clio_config::sync_key::load_all_base64(&|key: &str| get(key))
}

/// The keys the transfer path uses: the newest key then every archived key,
/// read from the explicit `CLIO_SYNC_KEY_FILE` when set, else from this node's
/// recorded store. A caller cannot supply a key here; the argument is rejected
/// before this is reached.
fn custody_cipher(get: &dyn Fn(&str) -> Option<String>) -> Result<Vec<SyncCipherKey>, AmError> {
    Ok(custody_secrets(get)?
        .iter()
        .map(|secret| SyncCipherKey::from_secret(secret))
        .collect())
}

/// The client's encryption keys for the status and ack-skip paths, newest
/// first. One JSON string keeps the old single-key calls working; a JSON array
/// of strings carries a rotation. An absent or null argument means no keys.
fn cipher_keys(args: &Value) -> Result<Vec<SyncCipherKey>, AmError> {
    let Some(raw) = args.get("sync_key").filter(|raw| !raw.is_null()) else {
        return Ok(Vec::new());
    };
    let secrets: Vec<&str> = match raw {
        Value::String(secret) => vec![secret.as_str()],
        Value::Array(entries) => entries
            .iter()
            .map(|entry| {
                entry.as_str().ok_or_else(|| {
                    AmError::new(
                        ErrorCode::InvalidArgument,
                        "sync_key entries must be strings",
                    )
                })
            })
            .collect::<Result<Vec<_>, AmError>>()?,
        _ => {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "sync_key must be a string or an array of strings",
            ));
        }
    };
    Ok(secrets
        .iter()
        .map(|secret| SyncCipherKey::from_secret(secret))
        .collect())
}

/// Banks argument: array of non-empty strings, or the connection default
/// (omitted, null, or empty array all resolve to the default bank).
fn banks_of(state: &McpState, args: &Value) -> Result<Vec<String>, AmError> {
    let Some(raw) = args.get("banks").filter(|r| !r.is_null()) else {
        return Ok(vec![state.default_bank.clone()]);
    };
    let list = raw.as_array().ok_or_else(|| {
        AmError::new(
            ErrorCode::InvalidArgument,
            "banks must be an array of strings",
        )
    })?;
    let banks: Vec<String> = list
        .iter()
        .map(|b| {
            let name = b.as_str().unwrap_or_default();
            if name.is_empty() {
                return Err(AmError::new(
                    ErrorCode::InvalidArgument,
                    "banks entries must be non-empty strings",
                ));
            }
            Ok(name.to_owned())
        })
        .collect::<Result<Vec<_>, AmError>>()?;
    if banks.is_empty() {
        return Ok(vec![state.default_bank.clone()]);
    }
    Ok(banks)
}

/// Mask a bearer token: full mask for short tokens, last-4 tail otherwise.
fn mask_token(token: Option<&String>) -> Option<String> {
    token.as_ref().map(|t| {
        let chars: Vec<char> = t.chars().collect();
        if chars.len() <= 8 {
            return "***".to_owned();
        }
        let tail: String = chars[chars.len() - 4..].iter().collect();
        format!("***{tail}")
    })
}

#[cfg(test)]
#[path = "sync_tools_test_support_tests.rs"]
mod sync_tools_test_support_tests;

#[cfg(test)]
#[path = "sync_tools_tests.rs"]
mod sync_tools_tests;

#[cfg(test)]
#[path = "sync_tools_key_tests.rs"]
mod sync_tools_key_tests;
