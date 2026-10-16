// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the injected event-writer faults the
//! failure scenarios need: an intent write that fails before any store
//! effect, and an outcome write that fails over a real store which still
//! records the intent.
//!
//! ## Owns
//! - `FaultEvents`, the two injected arms over `LocalEventStore`
//!
//! ## Does not own
//! - The erasure retry contract that consumes it (`clio-compliance`)
//! - The real durable event writer (`clio-store`)
//! - What each scenario asserts (`scenarios`)
//!
//! ## Boundary
//!
//! Changes to which write is injected should normally require changes here.
//! Changes to the retry flow should not.
//!
//! Keep this module focused on the two injected event-writer faults.

use clio::store::{LocalEventIntent, LocalEventStore, LocalEventView};
use clio::{AmError, ErrorCode};

/// An injected event-writer fault: an intent failure with no store behind it,
/// or an outcome failure over a real store that records the intent.
pub struct FaultEvents<'a> {
    inner: Option<&'a dyn LocalEventStore>,
    fail_intent: bool,
}

impl<'a> FaultEvents<'a> {
    /// An intent write that always fails before any effect.
    #[must_use]
    pub fn intent_fails() -> Self {
        Self {
            inner: None,
            fail_intent: true,
        }
    }

    /// A real store that records intents while every outcome write fails.
    #[must_use]
    pub fn outcome_fails_over(store: &'a dyn LocalEventStore) -> Self {
        Self {
            inner: Some(store),
            fail_intent: false,
        }
    }
}

impl LocalEventStore for FaultEvents<'_> {
    fn record_intent(&self, intent: &LocalEventIntent<'_>) -> Result<(), AmError> {
        if self.fail_intent {
            return Err(AmError::new(
                ErrorCode::Internal,
                "injected intent-write failure",
            ));
        }
        match self.inner {
            Some(inner) => inner.record_intent(intent),
            None => Ok(()),
        }
    }

    fn record_outcome(
        &self,
        _action_id: &str,
        _denied: bool,
        _result: &str,
    ) -> Result<(), AmError> {
        Err(AmError::new(
            ErrorCode::Internal,
            "injected outcome-write failure",
        ))
    }

    fn pending_actions(&self) -> Result<Vec<String>, AmError> {
        match self.inner {
            Some(inner) => inner.pending_actions(),
            None => Ok(Vec::new()),
        }
    }

    fn list_events(&self) -> Result<Vec<LocalEventView>, AmError> {
        match self.inner {
            Some(inner) => inner.list_events(),
            None => Ok(Vec::new()),
        }
    }

    fn prune_completed_before(&self, cutoff: &str) -> Result<usize, AmError> {
        match self.inner {
            Some(inner) => inner.prune_completed_before(cutoff),
            None => Ok(0),
        }
    }
}
