// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the MCP config/ranking surface:
//! routing `config_get`, `config_set`, `config_profiles`,
//! `config_profile_apply`, `ranking_env_get`, and `ranking_env_set` to the
//! `clio-config` `Runtime`.
//!
//! ## Owns
//! - Argument decode and result shaping for these six tool names
//! - The runtime lock (config tools serialize on `McpState.config`)
//! - The apply call after an applied `ranking_env_set` or a successful
//!   `config_profile_apply`, made only once the runtime guard is released
//!
//! ## Does not own
//! - Config resolution, validation, or ranking math (`clio-config` owns those;
//!   this module never re-implements them)
//! - Secret masking rules (`clio_config::secret`; every view is already masked)
//! - The retention-profile tools (see `retention_tools`)
//!
//! ## Boundary
//!
//! Changes to config/ranking argument decode or response shaping should
//! normally require changes here. Changes to config semantics should not
//! require changes here.
//!
//! Keep this module focused on the MCP config/ranking surface.

use crate::read_tools::tool_result_value;
use crate::runtime::McpState;
use clio_config::ranking::RankingEnvPatch;
use clio_config::{ConfigScope, Runtime};
use clio_types::{AmError, ErrorCode};
use serde_json::Value;

/// Route a read-side config/ranking tool.
pub(crate) fn dispatch_read(state: &McpState, name: &str, args: &Value) -> Result<Value, AmError> {
    let rt: std::sync::MutexGuard<'_, Runtime> =
        state.config.lock().expect("config runtime mutex poisoned");
    match name {
        "config_get" => tool_result_value(rt.config_get(path_of(args))),
        "config_profiles" => tool_result_value(rt.config_profiles()),
        "ranking_env_get" => tool_result_value(rt.ranking_env_get()),
        other => Err(AmError::not_implemented(other)),
    }
}

/// Route a mutating config/ranking tool.
pub(crate) fn dispatch_write(state: &McpState, name: &str, args: &Value) -> Result<Value, AmError> {
    state.authorize_admin()?;
    let mut rt: std::sync::MutexGuard<'_, Runtime> =
        state.config.lock().expect("config runtime mutex poisoned");
    match name {
        "config_set" => {
            let path = args.get("path").and_then(Value::as_str).unwrap_or("");
            let value = args.get("value").cloned().unwrap_or(Value::Null);
            let scope = ConfigScope::parse(scope_of(args))?;
            tool_result_value(rt.config_set(path, value, scope))
        }
        "config_profile_apply" => {
            let name = args.get("name").and_then(Value::as_str).unwrap_or_default();
            let outcome = tool_result_value(rt.config_profile_apply(name));
            // The apply path locks the same runtime, so the guard must be gone
            // before it runs. A failed apply changes no ranking environment.
            drop(rt);
            if outcome.is_ok() {
                state.apply_ranking_policy();
            }
            outcome
        }
        "ranking_env_set" => {
            let patch: RankingEnvPatch = match args.get("patch") {
                Some(p) => serde_json::from_value(p.clone()).map_err(|e| {
                    AmError::new(
                        ErrorCode::InvalidType,
                        format!("invalid ranking patch: {e}"),
                    )
                })?,
                None => RankingEnvPatch::default(),
            };
            let dry_run = args
                .get("dry_run")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let outcome = tool_result_value(rt.ranking_env_set(&patch, dry_run));
            // The apply path locks the same runtime, so the guard must be gone
            // before it runs. A failed set and a dry run apply nothing.
            drop(rt);
            if outcome.is_ok() && !dry_run {
                state.apply_ranking_policy();
            }
            outcome
        }
        other => Err(AmError::not_implemented(other)),
    }
}

/// Optional dotted config path; an omitted or empty path reads the whole
/// effective document (matching `clio-config` semantics).
fn path_of(args: &Value) -> Option<&str> {
    args.get("path").and_then(Value::as_str)
}

fn scope_of(args: &Value) -> &str {
    args.get("scope")
        .and_then(Value::as_str)
        .unwrap_or("session")
}

#[cfg(test)]
#[path = "config_tools_tests.rs"]
mod config_tools_tests;

#[cfg(test)]
#[path = "config_tools_trust_tests.rs"]
mod config_tools_trust_tests;
