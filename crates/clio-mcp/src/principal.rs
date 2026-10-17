// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for one resolved principal: the actor a network
//! credential acts as, the banks it may reach, and the single authorization rule
//! those two values imply for one tool call.
//!
//! ## Owns
//! - `Principal` (label, actor, bank list) and `session_identity`, the record
//!   identity a session binds to
//! - The two refusal rules applied to one call: a declared actor that disagrees
//!   with the bound one, and a bank outside the credential's list
//!
//! ## Does not own
//! - Where a principal comes from (see `http_auth`)
//! - Whether a presented session id belongs to the requester (see `http_guard`)
//! - The shared-bank deployment gate (see `runtime_context`)
//! - Tool bodies, which never learn that a principal exists
//!
//! ## Boundary
//!
//! Changes to what a credential is allowed to do should normally require changes
//! here. Changes to how a credential is read from disk should not require
//! changes here.
//!
//! Keep this module focused on the identity rule, not on transport or storage.
//!
//! # One resolver, two contracts
//!
//! A principal exists only where a credential named one. The local transports
//! have no credential edge — the process owner already holds the store — so
//! their calls stay unbound and this rule never runs for them.

use clio_types::{Actor, AmError, ErrorCode};
use serde_json::Value;

/// Who a credential names and what it may reach.
///
/// `label` is the operator-facing name of the record and is not secret: audit
/// reads may return it inside a recorded row's detail. It is never used as a
/// credential, and it is never the secret or a digest of one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Principal {
    pub(crate) label: String,
    pub(crate) actor: Actor,
    /// Exactly the banks the record named, in the order the record listed them.
    /// Ignored when `all_banks` is set.
    pub(crate) banks: Vec<String>,
    /// The local process owner reaches every bank because it already holds the
    /// whole store; a record loaded from disk never sets this.
    all_banks: bool,
}

impl Principal {
    /// Build a principal from a record's actor and bank list.
    pub(crate) fn new(label: String, actor: Actor, banks: Vec<String>) -> Self {
        Self {
            label,
            actor,
            banks,
            all_banks: false,
        }
    }

    /// Build the local process owner's principal.
    ///
    /// The loopback server mints this for itself when no `--token-file` was
    /// given. It is administrative and reaches every bank, matching the
    /// authority the process owner already has over the store; the bearer token
    /// is what proves the caller is that owner.
    pub(crate) fn local_owner(label: String, actor: Actor) -> Self {
        Self {
            label,
            actor,
            banks: Vec::new(),
            all_banks: true,
        }
    }

    /// The identity a session binds to: the record's label.
    ///
    /// The store refuses a repeated label, so within one server's store the
    /// label names exactly one credential. Two different credentials can
    /// therefore never compare equal, and no record can answer for another —
    /// which is what keeps a session bound to the one principal that minted it.
    ///
    /// Not a secret and never logged: the value lives in one process's session
    /// map and is only ever compared.
    pub(crate) fn session_identity(&self) -> String {
        self.label.clone()
    }

    /// The actor this principal authorizes a call as, given what the caller
    /// declared in its arguments.
    ///
    /// An absent declaration is not a conflict: the bound actor is used. An
    /// equal declaration is accepted rather than special-cased away, because it
    /// cannot mislead. Any other value is refused, including a value that is not
    /// a valid actor name at all: this path must not become a way to enumerate
    /// the accepted actor names.
    ///
    /// # Errors
    /// [`ErrorCode::Forbidden`] when the declared actor differs from the bound one.
    pub(crate) fn authorize_actor(&self, declared: Option<&str>) -> Result<Actor, AmError> {
        match declared {
            None => Ok(self.actor),
            Some(raw) if raw == self.actor.as_str() => Ok(self.actor),
            Some(raw) => Err(AmError::new(
                ErrorCode::Forbidden,
                format!(
                    "actor `{raw}` is not the actor this credential names (`{}`); \
                     the credential decides the actor, so drop the argument or send its own",
                    self.actor
                ),
            )),
        }
    }

    /// Refuse a bank this credential's list does not name.
    ///
    /// There is no fallback: falling back would write to a bank the caller did
    /// not name, which is worse than a refusal, because the caller would see
    /// success while the data landed elsewhere.
    ///
    /// # Errors
    /// [`ErrorCode::Forbidden`] when `bank` is outside the list.
    pub(crate) fn authorize_bank(&self, bank: &str) -> Result<(), AmError> {
        if self.all_banks || self.banks.iter().any(|b| b == bank) {
            return Ok(());
        }
        Err(AmError::new(
            ErrorCode::Forbidden,
            format!("this credential may not reach bank `{bank}`"),
        ))
    }

    /// Apply both rules to one call's arguments and the bank the call resolved
    /// to, returning the actor the call runs as.
    ///
    /// # Errors
    /// [`ErrorCode::Forbidden`] for either conflict.
    pub(crate) fn authorize(&self, args: &Value, resolved_bank: &str) -> Result<Actor, AmError> {
        let actor = self.authorize_actor(args.get("actor").and_then(Value::as_str))?;
        self.authorize_bank(resolved_bank)?;
        Ok(actor)
    }
}

#[cfg(test)]
#[path = "principal_tests.rs"]
mod principal_tests;
