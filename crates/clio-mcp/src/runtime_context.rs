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
//! - `next_item_id` / `resolve_ctx` / `resolve_timestamp` / `require_bank_grant`
//!   on `McpState`
//! - `dispatch_maintenance` (off-path maintenance handoff)
//! - `now_utc` (RFC 3339 UTC stamping; the calendar arithmetic is the shared
//!   `clio_types::civil_from_days`)
//!
//! ## Does not own
//! - Share-path semantics or the shared-bank deployment permission (`runtime`)
//! - The credential's bank list itself (`principal`)
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
use clio_write::{
    MAINTENANCE_DRAIN_BUDGET, MaintenanceSignal, SharedMaintenanceHook, dispatch_async,
};
use serde_json::Value;
use std::sync::Arc;

impl McpState {
    /// Generate a unique item id for caller-omitted ids.
    ///
    /// Unique across processes, not only within one: a per-process counter
    /// alone restarts at 1 in every fresh CLI one-shot, so a bare `itm-mcp-{n}`
    /// repeats `itm-mcp-1` on every later invocation and collides on the items
    /// primary key. Mix the process id, a nanosecond clock, and the in-process
    /// counter, the same shape [`Self::next_clean_batch_id`] uses.
    #[must_use]
    pub fn next_item_id(&self) -> String {
        format!(
            "itm-mcp-{}-{}-{}",
            std::process::id(),
            unix_nanos(),
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
        format!(
            "hclean-{}-{}-{}",
            std::process::id(),
            unix_nanos(),
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
        // Note the resolved bank so the background trigger sweeps it on its
        // next tick instead of listing every bank in the store.
        self.maintenance_trigger.note_bank(bank);
        Ok(ctx)
    }

    /// Refuse a bank-scoped read when the bank is unknown.
    ///
    /// The configured default and enabled shared bank are valid even when
    /// empty; custom banks exist when registered or used by durable data.
    ///
    /// # Errors
    /// `not_found` for an unknown bank, or storage failures.
    pub fn require_known_bank(&self, bank: &str) -> Result<(), AmError> {
        if bank == self.default_bank || (bank == SHARED_BANK && self.shared_enabled) {
            return Ok(());
        }
        if self.ops_store.bank_exists(bank)? {
            return Ok(());
        }
        Err(AmError::new(
            clio_types::ErrorCode::NotFound,
            format!("bank `{bank}` not found"),
        ))
    }

    /// Refuse an operator scope the bound credential's grant does not cover.
    ///
    /// Some operator tools resolve their scope after the request is authorized:
    /// a registered plan names its own bank, and a bank-less scope would
    /// otherwise widen to every bank in the store. The grant must therefore
    /// cover the scope the tool actually acts on, not only the `bank` argument.
    /// An unbound local caller is allowed, because the process owner already
    /// holds the store, and the deployment's shared-bank permission is never
    /// consulted: enablement is not a grant.
    ///
    /// The message names neither the bank nor a plan id, so a caller that
    /// omitted `bank` cannot learn from the refusal that the scope it reached
    /// exists.
    ///
    /// # Errors
    /// [`ErrorCode::Forbidden`] when a bound credential's list omits `bank`.
    pub(crate) fn require_bank_grant(&self, bank: &str) -> Result<(), AmError> {
        let Some(principal) = &self.principal else {
            return Ok(());
        };
        if principal.covers_bank(bank) {
            return Ok(());
        }
        Err(AmError::new(
            ErrorCode::Forbidden,
            "the operator scope for this request is outside this credential's banks",
        ))
    }

    /// Resolve the effective timestamp: a non-empty `args.timestamp` wins over now.
    ///
    /// The value is returned in the store's canonical form — UTC RFC 3339 at
    /// second precision — because every item time column is compared as text.
    /// Doing it here, at the one resolver every write tool shares, keeps a
    /// caller's fractional or offset stamp from reaching a column the rest of
    /// the store compares as `YYYY-MM-DDThh:mm:ssZ`.
    ///
    /// # Errors
    /// [`ErrorCode::InvalidArgument`] when the caller's value is not a stamp
    /// this build can place: it is refused rather than stored verbatim, since a
    /// literal string in a time column compares as one.
    pub fn resolve_timestamp(&self, args: &Value) -> Result<String, AmError> {
        let raw = args
            .get("timestamp")
            .and_then(Value::as_str)
            .filter(|t| !t.is_empty())
            .map_or_else(now_utc, str::to_owned);
        crate::runtime_context::canonical_write_stamp(&raw)
    }

    /// Dispatch maintenance signals, then wait for the hook to consume them.
    ///
    /// Structural refresh stays off this path; what is waited for is the hook's
    /// own work — attaching the leaf and upserting the rows the attach changed.
    /// Without that wait a one-shot write can exit before its leaf rows land, so
    /// a later read finds nothing. The wait is bounded by
    /// [`MAINTENANCE_DRAIN_BUDGET`], so a stuck store costs one write that
    /// budget instead of hanging it.
    ///
    /// A wait that expires, or a hook that panicked, returns `false`, and that
    /// verdict says the rows this write produced may never have landed. The
    /// write itself already succeeded and is not rolled back; the verdict is
    /// recorded on the engine instead, so the degradation reaches the process
    /// log now and `maintenance status` later.
    pub fn dispatch_maintenance(&self, signals: Vec<MaintenanceSignal>) {
        let job = dispatch_async(Arc::clone(&self.maint) as SharedMaintenanceHook, signals);
        if let Some(job) = job
            && !job.drain(MAINTENANCE_DRAIN_BUDGET)
        {
            self.maint.record_persist_failure(&AmError::new(
                ErrorCode::Internal,
                format!(
                    "maintenance hook did not finish within {}ms, so its memtree rows may be unwritten",
                    MAINTENANCE_DRAIN_BUDGET.as_millis()
                ),
            ));
        }
    }
}

/// The canonical form of a caller-supplied stamp, or the refusal for one this
/// build cannot place.
///
/// A stamp that cannot be placed (not RFC 3339, a month the year does not have,
/// an offset of `+24:00`) would be stored and compared as a literal string, so
/// it is refused with the same invalid-argument code the argument contract uses
/// rather than written.
///
/// # Errors
/// [`ErrorCode::InvalidArgument`] naming the value that cannot be placed.
pub(crate) fn canonical_write_stamp(raw: &str) -> Result<String, AmError> {
    clio_store::canonical_write_stamp(raw)
}

/// Nanoseconds since the Unix epoch, or `0` when the clock predates it.
///
/// One source for the process-unique id minting above: a caller-omitted item
/// id and a `hygiene_clean` batch id both need a clock reading that differs
/// between two fresh processes.
fn unix_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
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
