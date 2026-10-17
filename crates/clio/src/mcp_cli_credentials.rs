// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for loading the `clio mcp http` credential store
//! the `--token-file` flag names: decoding the flag, resolving its path, and
//! refusing every unusable state before the listener is bound.
//!
//! ## Owns
//! - `load_http_credentials`: the fail-closed read of the operator-named store
//!
//! ## Does not own
//! - The `clio mcp http` handler and its other flags (see `mcp_cli`)
//! - The record shape, its validation, and the store file itself (see
//!   `clio_config::auth_tokens`)
//! - The credential edge over a live request (see `clio-mcp`'s `http_auth`)
//!
//! ## Boundary
//!
//! Changes to which store `--token-file` loads, or to its refusal rules, should
//! normally require changes here. Changes to how a transport authorizes a
//! request should not.
//!
//! Keep this module focused on one flag's store load.

/// Load the credential store `--token-file` names.
///
/// `None` when the flag is absent; the loopback default is then a minted
/// one-time token (see `mcp_cli`), not an unauthenticated server. An explicit
/// flag whose path is absent, unreadable, malformed, not owner-only, or holds
/// no record refuses the start: an operator who named a store asked for that
/// identity, and an unusable one would answer every request with a 401.
///
/// The environment lookup is a parameter so a test never reads the host
/// environment and the production caller and the tests drive the same code.
pub(crate) fn load_http_credentials<F>(
    flags: &std::collections::HashMap<String, String>,
    get: &F,
) -> Result<Option<std::sync::Arc<clio_mcp::http_auth::CredentialStore>>, String>
where
    F: Fn(&str) -> Option<String>,
{
    let Some(explicit) = flags.get("token-file") else {
        return Ok(None);
    };
    let Some(path) = clio_config::auth_tokens::resolve_token_store_path_with(
        Some(std::path::PathBuf::from(explicit)),
        get,
    ) else {
        return Err("--token-file named a path that cannot be resolved".into());
    };
    let store = clio_mcp::http_auth::CredentialStore::load(&path).map_err(|e| e.message)?;
    if store.is_empty() {
        return Err(format!(
            "credential store {} holds no record; add one with `clio auth token add`",
            path.display()
        ));
    }
    Ok(Some(std::sync::Arc::new(store)))
}

#[cfg(test)]
#[path = "mcp_cli_credentials_tests.rs"]
mod credentials_tests;
