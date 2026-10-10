// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the MCP runtime's per-call helpers: unique id
//! generation, bank/actor context resolution, timestamp defaults, and the
//! non-blocking maintenance dispatch hook.
//!
//! ## Owns
//! - `next_item_id` / `resolve_ctx` / `resolve_timestamp` on `McpState`
//! - `dispatch_maintenance` (off-path maintenance handoff)
//! - `now_utc` (RFC 3339 UTC stamping; the calendar arithmetic is the shared
//!   `clio_types::civil_from_days`)
//!
//! ## Does not own
//! - Share-path semantics or the shared-bank deployment permission (`runtime`)
//! - Maintenance consumers (`clio-write`)
//!
//! ## Boundary
//!
//! Changes to per-call context resolution or timestamp defaults should normally
//! require changes here. Changes to `McpState` construction should not.
//!
//! Keep this module focused on runtime call-context helpers.

use crate::runtime::{McpState, SHARED_BANK};
use clio_types::{Actor, AmError, Bank, ErrorCode, OpContext, PrincipalLabel, civil_from_days};
use clio_write::{MaintenanceSignal, SharedMaintenanceHook, dispatch_async};
use serde_json::Value;
use std::sync::Arc;

impl McpState {
    /// Generate a unique item id for caller-omitted ids.
    #[must_use]
    pub fn next_item_id(&self) -> String {
        format!(
            "itm-mcp-{}",
            self.id_counter
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        )
    }

    /// Generate an invocation-unique `hygiene_clean` batch id.
    ///
    /// Dry-run and confirmed clean rows are durable and keyed by the batch id,
    /// so the id must not repeat across processes: a per-process counter alone
    /// regenerates the same value in every fresh process and collides on the
    /// hygiene-log primary key. Mix the process id, a nanosecond clock, and the
    /// in-process counter.
    #[must_use]
    pub fn next_clean_batch_id(&self) -> String {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        format!(
            "hclean-{}-{nanos}-{}",
            std::process::id(),
            self.id_counter
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        )
    }

    /// Resolve bank/actor context.
    ///
    /// Two contracts share this one function, and the difference is the
    /// principal, never a second resolver:
    ///
    /// The bound path also carries the credential's label on the returned
    /// context, so an audited row can name which credential acted. The unbound
    /// path returns a context with no label.
    ///
    /// - **Bound** (a credential named an actor and a bank list): the actor comes
    ///   from the credential. A declared actor that disagrees is refused and an
    ///   absent one uses the bound value, so the audit rows cannot claim an
    ///   actor the caller did not present. The resolved bank must be inside the
    ///   credential's list, whether the caller named it or the default resolved
    ///   to it, and an out-of-list bank is refused rather than replaced by a
    ///   listed one.
    /// - **Unbound** (no credential): args win over the session defaults, exactly
    ///   as before. The local transports use this, because a local process owner
    ///   already holds the store.
    ///
    /// The shared-bank deployment gate runs last and is unchanged, so it applies
    /// on top of the credential's list rather than instead of it.
    ///
    /// # Errors
    /// On the bound path, [`ErrorCode::Forbidden`] for an actor conflict or an
    /// out-of-list bank. On the unbound path, [`ErrorCode::MissingBank`] for an
    /// empty resolved bank and [`ErrorCode::InvalidArgument`] for an unknown
    /// `actor` value. Both paths refuse a `shared` bank when the deployment has
    /// not enabled the shared surface.
    pub fn resolve_ctx(&self, args: &Value) -> Result<OpContext, AmError> {
        let bank = args
            .get("bank")
            .and_then(Value::as_str)
            .unwrap_or(&self.default_bank);
        let actor = match &self.principal {
            Some(principal) => principal.authorize(args, bank)?,
            None => match args.get("actor").and_then(Value::as_str) {
                Some(raw) => Actor::parse(raw)?,
                None => self.default_actor,
            },
        };
        // Bank-level authorization: the shared namespace is reachable only when
        // the deployment enabled it, no matter which tool names the bank.
        if bank == SHARED_BANK && !self.shared_enabled {
            return Err(AmError::new(
                ErrorCode::Forbidden,
                format!(
                    "bank `{SHARED_BANK}` requires the deployment to enable the shared bank surface"
                ),
            ));
        }
        // The bound label rides on the context so an audited row can name the
        // credential beside the role it claims. Only a bound transport has one:
        // the unbound branch above resolved the actor from the caller's own
        // arguments and there is no credential to name.
        let ctx = match &self.principal {
            Some(principal) => OpContext::with_principal(
                Bank::new(bank),
                actor,
                PrincipalLabel::new(principal.label.clone()),
            ),
            None => OpContext::new(Bank::new(bank), actor),
        };
        if ctx.bank.as_str().is_empty() {
            return Err(AmError::new(ErrorCode::MissingBank, "bank is required"));
        }
        Ok(ctx)
    }

    /// Resolve the effective timestamp: a non-empty `args.timestamp` wins over now.
    #[must_use]
    pub fn resolve_timestamp(&self, args: &Value) -> String {
        args.get("timestamp")
            .and_then(Value::as_str)
            .filter(|t| !t.is_empty())
            .map_or_else(now_utc, str::to_owned)
    }

    /// Dispatch maintenance signals off the write path (never gates the response).
    pub fn dispatch_maintenance(&self, signals: Vec<MaintenanceSignal>) {
        if !signals.is_empty() {
            dispatch_async(Arc::clone(&self.maint) as SharedMaintenanceHook, signals);
        }
    }
}

/// UTC `YYYY-MM-DDThh:mm:ssZ` for a Unix second count. Split out from
/// [`now_utc`] so the stamp can be asserted at a known month end rather than
/// only at whatever second a test happens to run in.
pub(super) fn utc_stamp(seconds: u64) -> String {
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

/// Current UTC time formatted RFC 3339 (seconds precision; caller-omitted
/// timestamp default).
#[must_use]
pub fn now_utc() -> String {
    utc_stamp(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs()),
    )
}

#[cfg(test)]
#[path = "runtime_context_tests.rs"]
mod runtime_context_tests;
