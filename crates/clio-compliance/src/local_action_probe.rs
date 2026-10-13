// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the local protected-action probe: a proof
//! driver that runs one synthetic protected action against the durable local
//! event writer, sequencing the intent write, the effect, and the outcome
//! write so the refusal, pending-reconciliation, and prune semantics can be
//! proven end to end.
//!
//! ## Owns
//! - `LocalActionProbe`, its `ProbeAction` input, and its `ProbeOutcome`
//!   result
//! - The fixed event kind label every probe action writes
//!
//! ## Does not own
//! - Any real export, erase, or restore entry point: no production path calls
//!   this module, and it wires none of them
//! - The durable writer and its SQL (`clio-store` owns the local event store)
//! - Any real protected effect: the effect is the caller-supplied closure, and
//!   its result text is the caller's redaction responsibility
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Changes to how the probe sequences the intent, effect, and outcome writes
//! should normally require changes here. Changes to what the writer persists
//! or to real protected actions should not require changes here.
//!
//! Keep this module focused on the protected-action proof sequence.

use clio_store::{LocalEventIntent, LocalEventStore};
use clio_types::AmError;
use std::sync::Arc;

/// The event kind label every probe action writes.
pub const PROBE_KIND: &str = "local_action_probe";

/// One protected action a probe run performs.
pub struct ProbeAction<'a> {
    /// Caller-unique id for the action; the writer keys on it.
    pub action_id: &'a str,
    /// Attribution actor.
    pub actor: &'a str,
    /// Opaque scope string, stored verbatim.
    pub scope: &'a str,
    /// Event time (UTC text), caller-supplied so tests control the clock.
    pub event_time: &'a str,
}

/// What one probe run did with its action.
#[derive(Debug)]
pub enum ProbeOutcome {
    /// The intent write failed, so the action was refused before any effect.
    Refused {
        /// Why the intent write failed.
        cause: AmError,
    },
    /// The effect ran once and the outcome write completed the record.
    Completed,
    /// The effect ran once and the outcome write failed; the pending record
    /// stays durable for reconciliation by action id.
    Pending {
        /// Why the outcome write failed.
        cause: AmError,
    },
}

/// Proof driver for the durable local event writer.
///
/// The probe writes the intent, runs the protected effect at most once, and
/// completes the record with the outcome. An intent failure refuses the
/// action with zero effects; an outcome failure after the effect leaves the
/// pending record durable for reconciliation by action id.
pub struct LocalActionProbe {
    store: Arc<dyn LocalEventStore>,
}

impl LocalActionProbe {
    /// A probe over one durable writer.
    #[must_use]
    pub fn new(store: Arc<dyn LocalEventStore>) -> Self {
        Self { store }
    }

    /// Run one protected action against the writer.
    ///
    /// The effect runs only after the intent write succeeds, and at most
    /// once. Its `Ok` result is recorded as allowed; its `Err` reason is
    /// recorded as a denial. Both texts must stay generic.
    pub fn run(
        &self,
        action: &ProbeAction<'_>,
        effect: impl FnOnce() -> Result<String, String>,
    ) -> ProbeOutcome {
        let intent = LocalEventIntent {
            action_id: action.action_id,
            actor: action.actor,
            scope: action.scope,
            kind: PROBE_KIND,
            event_time: action.event_time,
        };
        // The intent is durable before the effect: any failure here refuses
        // the action, and nothing durable is left half-written.
        if let Err(cause) = self.store.record_intent(&intent) {
            return ProbeOutcome::Refused { cause };
        }
        let (denied, result) = match effect() {
            Ok(result) => (false, result),
            Err(reason) => (true, reason),
        };
        match self.store.record_outcome(action.action_id, denied, &result) {
            Ok(()) => ProbeOutcome::Completed,
            Err(cause) => ProbeOutcome::Pending { cause },
        }
    }
}

#[cfg(test)]
#[path = "local_action_probe_tests.rs"]
mod local_action_probe_tests;
