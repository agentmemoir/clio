// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the MCP-bound sync transfer tools
//! (`sync_push` / `sync_pull`, including the bidirectional mode): client
//! config assembly from custody, token masking, and the journal-backed
//! erasure-denial authority every pulled mutation passes before it can write.
//!
//! ## Owns
//! - JSON argument decoding and result shaping for the transfer tools
//! - The pull and bidirectional runs under `pull_with_denial` /
//!   `bidirectional_with_denial`, so a denied subject is dead-lettered before
//!   it can become readable and a journal this host cannot resolve refuses
//!   the run instead of applying it unguarded
//! - The transfer key read: the explicit `CLIO_SYNC_KEY_FILE` first, else the
//!   recorded store
//! - Bearer-token masking on the transfer response (last-4 at most)
//!
//! ## Does not own
//! - Push feed packaging, cursors, and conflict/DLQ semantics (`clio-sync`)
//! - Bank argument decoding and the shared client config (`sync_tools`)
//! - The serve route's own journal authority wiring (`sync_tools`)
//!
//! ## Boundary
//!
//! Changes to the transfer tools' arguments, keys, or denial guard should
//! normally require changes here. Changes to sync protocol semantics should
//! not require changes here.
//!
//! Keep this module focused on the transfer tools and their denial guard.

use crate::runtime::{McpState, now_utc};
use crate::sync_tools::{banks_of, client_cfg, required_custody_cipher};
use crate::write_tools::opt_str;
use clio_compliance::JournalDenialProbe;
use clio_sync::client::{SyncModes, bidirectional_with_denial, pull_with_denial, push};
use clio_types::{AmError, ErrorCode};
use serde_json::{Value, json};

/// `sync_push` / `sync_pull(remote, mode?, banks?, token?)`.
///
/// The tool name fixes the default mode; an explicit `mode` argument overrides
/// it (`bidirectional` pulls then pushes in one operator action). The remote
/// bearer token is echoed masked (last 4 at most), never in the clear.
#[inline(never)]
pub(crate) fn transfer(
    state: &McpState,
    args: &Value,
    default_mode: &str,
) -> Result<Value, AmError> {
    transfer_with(state, args, default_mode, &|key| std::env::var(key).ok())
}

/// The transfer run for one environment lookup, so a test can pin the custody
/// keys and the resolved denial journal without reading the machine's own
/// environment.
pub(crate) fn transfer_with(
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
    let mut cfg = client_cfg(state, opt_str(args, "remote"), opt_str(args, "token"));
    cfg.cipher = required_custody_cipher(get)?;
    let banks = banks_of(state, args)?;
    let mode = SyncModes::parse(opt_str(args, "mode").unwrap_or(default_mode))?;
    let rows = match mode {
        SyncModes::Push => push(&cfg, &banks)?,
        // The pull halves apply remote content, so they run under the journal
        // authority: a denied subject is dead-lettered before it can become
        // readable, and an authority this host cannot resolve refuses.
        SyncModes::Pull => {
            let authority = denial_authority(state, get)?;
            pull_with_denial(&cfg, &banks, Some(&authority))?
        }
        SyncModes::Bidirectional => {
            let authority = denial_authority(state, get)?;
            bidirectional_with_denial(&cfg, &banks, Some(&authority))?
        }
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

/// The journal-backed erasure-denial authority one pulled mutation passes
/// before it can write.
///
/// The resolution is [`JournalDenialProbe::resolved`], the same rule and
/// refusal the serve route applies: the journal path the host named, over the
/// deployment resolution (`CLIO_ERASURE_JOURNAL_FILE`, else the data
/// directory). A deployment that cannot name its journal refuses the pull
/// rather than applying it unguarded; a resolved path with no file yet is
/// clear.
///
/// # Errors
/// `ConfigCorrupt` when no journal path resolves, or when a host-named path
/// differs from the deployment resolution.
fn denial_authority(
    state: &McpState,
    get: &dyn Fn(&str) -> Option<String>,
) -> Result<JournalDenialProbe, AmError> {
    JournalDenialProbe::resolved(state.journal_path.as_deref(), get)
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
#[path = "sync_transfer_tests.rs"]
mod sync_transfer_tests;
