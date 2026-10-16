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
//! - `start_serve`: option decoding, the anonymous-bind refusal, the bound
//!   address payload, and the denial-aware serve entry point
//! - `denial_probe`: the journal probe built from the resolved journal path
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
use clio_store::ErasureDenial;
use clio_sync::prelude::{SyncServer, SyncServerOptions, sync_serve, sync_serve_with_denial};
use clio_types::{AmError, ErrorCode};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;

use crate::cli_error::CliError;

/// Start the sync server for one decoded `sync_serve` call.
///
/// Mirrors the bound tool's argument decoding and its refusal of an anonymous
/// bind outside dev mode. A resolvable erasure-journal path serves the server
/// through the denial-aware entry point with that journal probe; a deployment
/// with no resolvable journal path starts the server exactly as before.
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
    let banks = serve_banks(state, args);
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
    };
    let server = match denial_probe(env) {
        Some(probe) => sync_serve_with_denial(options, probe),
        None => sync_serve(options),
    }
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

/// The deployment's erasure-denial authority, when its journal path resolves.
/// Resolution refusal is the no-journal deployment: the server still starts.
fn denial_probe(env: &dyn Fn(&str) -> Option<String>) -> Option<Arc<dyn ErasureDenial>> {
    let path = clio_config::resolve_journal_path(&|key: &str| env(key)).ok()?;
    Some(Arc::new(JournalDenialProbe::new(path)) as Arc<dyn ErasureDenial>)
}

/// The banks one serve call covers: the decoded `banks` argument, else the
/// state's default bank.
fn serve_banks(state: &McpState, args: &Value) -> Vec<String> {
    let banks: Vec<String> = args
        .get("banks")
        .and_then(Value::as_array)
        .map(|banks| {
            banks
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    if banks.is_empty() {
        vec![state.default_bank.clone()]
    } else {
        banks
    }
}
