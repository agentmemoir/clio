// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for starting the `clio sync serve` process: the
//! server options decoded from the verb's arguments, and the deployment's
//! erasure-denial authority the server is served with.
//!
//! ## Owns
//! - `start_serve`: option decoding (through the bound tool's own bank
//!   decoder), the anonymous-bind refusal, and the bound address payload
//! - `denial_probe`: the journal-backed authority the server is served with,
//!   or the refusal when this deployment cannot name its journal
//! - `serve_sync_key`: the sync key the peer serves with, read from the
//!   explicit key file or the recorded store
//!
//! ## Does not own
//! - The verb, its flags, dispatch, or views (see `sync_cli`)
//! - The server and its denial guards (`clio-sync`)
//! - Where the journal path lives (`clio-config`)
//!
//! ## Boundary
//!
//! Changes to how the serve process starts, or to which authority it is served
//! with, should normally require changes here. Changes to the serve verb's
//! flags or text view should not.
//!
//! Keep this module focused on starting one serve process.

use clio_compliance::JournalDenialProbe;
use clio_mcp::runtime::McpState;
use clio_mcp::sync_tools::banks_of;
use clio_store::ErasureDenial;
use clio_sync::prelude::{SyncCipherKey, SyncServer, SyncServerOptions, sync_serve_with_denial};
use clio_types::{AmError, ErrorCode};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::cli_error::CliError;

/// Start the sync server for one decoded `sync_serve` call.
///
/// The arguments are decoded through the bound tool's own bank decoder, so the
/// verb and the tool cannot disagree about the banks a serve call covers, and
/// the anonymous bind outside dev mode is refused exactly as the tool refuses
/// it. The server is always served with the deployment's journal-backed
/// erasure-denial authority: a deployment whose journal path cannot be resolved
/// is refused here, because serving unguarded would ignore recorded denials.
pub(crate) fn start_serve(
    state: &McpState,
    args: &Value,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<(SyncServer, Value), CliError> {
    let dev = args.get("dev").and_then(Value::as_bool).unwrap_or(false);
    let token_file = args
        .get("token_file")
        .and_then(Value::as_str)
        .map(PathBuf::from);
    if token_file.is_none() && !dev {
        return Err(CliError::from_am_error(&AmError::new(
            ErrorCode::Forbidden,
            "sync_serve requires auth material outside dev mode (refusing anonymous bind)",
        )));
    }
    let banks = banks_of(state, args).map_err(|error| CliError::from_am_error(&error))?;
    let ttl_secs = args.get("ttl_secs").and_then(Value::as_u64);
    let bind = args
        .get("bind")
        .and_then(Value::as_str)
        .unwrap_or("127.0.0.1:0");
    let options = SyncServerOptions {
        bind: bind.to_owned(),
        store: state.store.clone(),
        sync: state.sync.clone(),
        kms: state.kms.clone(),
        banks: banks.clone(),
        token_file,
        dev_mode: dev,
        ttl_secs,
        deadline_secs: None,
        device_id: state.device_id.clone(),
        sync_key: serve_sync_key(env)?,
    };
    let server = sync_serve_with_denial(options, denial_probe(env)?)
        .map_err(|error| CliError::from_am_error(&error))?;
    let payload = json!({
        "ok": true,
        "addr": server.addr(),
        "ttl_secs": ttl_secs.unwrap_or(clio_sync::SERVE_TTL_DEFAULT_SECS),
        "banks": banks,
        "dev": dev,
        "timestamp": clio_sync::clock::now_utc(),
    });
    Ok((server, payload))
}

/// The deployment's erasure-denial authority: the journal this deployment
/// resolves, or the refusal that keeps a serve route from running without it.
///
/// An unresolvable journal path is a refusal, not a fallback: a serve process
/// started without the authority would ignore every denial recorded against a
/// bank it serves. A journal file that does not exist yet is not this case —
/// the path resolves, and a missing journal is a clear one.
fn denial_probe(env: &dyn Fn(&str) -> Option<String>) -> Result<Arc<dyn ErasureDenial>, CliError> {
    let probe =
        JournalDenialProbe::resolved(None, env).map_err(|error| CliError::from_am_error(&error))?;
    Ok(Arc::new(probe) as Arc<dyn ErasureDenial>)
}

/// The sync key this peer serves with, when the deployment holds one.
///
/// The key is read through the custody route the transfer path uses — the
/// explicit `CLIO_SYNC_KEY_FILE` first, then this node's recorded store, newest
/// key first — so a serve process holds the same key the pushes it receives were
/// sealed under. A node that recorded no key holds none, and then serves sealed
/// rows exactly as it did before: it cannot open them, so it cannot resolve
/// their subject either.
///
/// A configured key that cannot be read is a refusal, because a serve process
/// that silently lost its key would apply every erased subject it was sent.
fn serve_sync_key(
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<Option<Arc<SyncCipherKey>>, CliError> {
    let explicit = env(clio_config::SYNC_KEY_FILE_ENV)
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    let secrets = match explicit {
        Some(path) => vec![
            clio_config::sync_key::read_key_file_base64(Path::new(&path))
                .map_err(|error| CliError::from_am_error(&error))?,
        ],
        None => clio_config::sync_key::load_all_base64(&|key: &str| env(key))
            .map_err(|error| CliError::from_am_error(&error))?,
    };
    Ok(secrets
        .first()
        .map(|secret| Arc::new(SyncCipherKey::from_secret(secret))))
}
