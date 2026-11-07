// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tally of tool calls one runtime state
//! served directly, keyed by the name of the tool that actually ran.
//!
//! ## Owns
//! - The per-state counter and the mutex that guards it
//! - Recording one execution, and reading the whole tally back ordered by name
//!
//! ## Does not own
//! - Which calls count as executed (see the shared execute path in `protocol`)
//! - Tool dispatch, argument checking, or result shaping (see `write_tools`)
//!
//! ## Boundary
//!
//! Changes to what the tally holds, or how it is keyed, should normally require
//! changes here. Changes to when a call is recorded should not: the execute path
//! decides that, and this module only keeps the count.
//!
//! Keep this module focused on counting executed calls.

use std::collections::BTreeMap;
use std::sync::Mutex;

/// The tool calls one runtime state served directly, counted by tool name.
///
/// Every transport hands its handler one state, and a per-request state is a
/// shallow clone, so the tally is shared exactly as the store and the engines
/// are. A count is per state: two states opened by one process count apart.
#[derive(Debug, Default)]
pub struct DirectCallAccounting {
    counts: Mutex<BTreeMap<String, u64>>,
}

impl DirectCallAccounting {
    /// Count one execution of `name`.
    pub(crate) fn record(&self, name: &str) {
        let mut counts = self.counts.lock().expect("call accounting mutex poisoned");
        *counts.entry(name.to_owned()).or_insert(0) += 1;
    }

    /// The tally so far, ordered by tool name.
    ///
    /// Only the tests read it today; the surface that reports the tally is not
    /// wired to a caller yet, so a production build holds this one unused.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn snapshot(&self) -> BTreeMap<String, u64> {
        self.counts
            .lock()
            .expect("call accounting mutex poisoned")
            .clone()
    }
}

#[cfg(test)]
#[path = "call_accounting_tests.rs"]
mod call_accounting_tests;
