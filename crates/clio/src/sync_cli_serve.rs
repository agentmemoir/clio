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
use clio_sync::prelude::{SyncServer, SyncServerOptions, sync_serve_with_denial};
use clio_types::{AmError, ErrorCode};
use serde_json::{Value, json};
use std::path::PathBuf;
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
