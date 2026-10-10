// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This crate is responsible for the persona companion object: the three
//! `persona_*` tools, document assembly, and the always-on budget.
//!
//! ## Owns
//! - `persona_get` / `persona_put_stable` / `persona_observe_preference` handlers
//! - Discrete (invalidation) and continuous (thin adapter) path split
//! - Persona token budget with score-ranked truncation
//! - `PersonaEvent` mutator telemetry (actor, bank, key, path)
//!
//! ## Does not own
//! - Persona persistence (`clio-store::PersonaStore`) or DEK crypto
//! - EMA math (the shared engine behind `clio-write::persona::observe_continuous`)
//! - Compose packing (`clio-retrieve::compose`, which consumes this crate's entries)
//!
//! ## Boundary
//!
//! Changes to tool semantics, path split, or budget policy should normally
//! require changes here. Changes to persona SQL or the EMA formula should not.
//!
//! # Persona vs `category=persona` boundary (documented, non-overlapping)
//!
//! The companion document is assembled **only** from the dedicated persona
//! tables (`persona_stable_entries` + `persona_preferences`), under the persona
//! token budget. Semantic `store(..., category=persona)` items are a separate
//! generic-store subsystem surfaced through retrieval; they are never merged
//! into `persona_get` and therefore cannot bypass the companion budget. The two
//! paths share no identity space: companion keys live in persona tables, store
//! items live in `items` with `category='persona'`.

#![forbid(unsafe_code)]

use clio_types::{AmError, OpContext};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

/// Default persona-channel token budget.
pub const DEFAULT_PERSONA_TOKEN_BUDGET: usize = 400;

pub mod budget;
pub mod document;
pub mod observe;
pub mod stable;


pub use budget::{PersonaPackEntry, truncate_document};
pub use document::{PersonaGetArgs, PersonaGetOutcome, persona_get};
pub use observe::{
    PersonaObserveOutcome, PersonaObservePreferenceArgs, persona_observe_preference,
};
pub use stable::{PersonaPutStableArgs, PersonaPutStableOutcome, persona_put_stable};

/// Telemetry for persona mutators (spirit; keys + redacted meta
/// only — never raw persona values). Reads (`persona_get`) emit nothing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PersonaEvent {
    /// `persona_put_stable` | `persona_observe_preference` (mutator-only).
    pub operation: String,
    /// `discrete` | `continuous` (the path split).
    pub path: String,
    /// `admitted` | `rejected` | `observed`.
    pub outcome: String,
    /// Bank isolation key.
    pub bank: String,
    /// Attribution actor.
    pub actor: String,
    /// Persona entry key.
    pub key: String,
    /// Caller timestamp.
    pub timestamp: String,
    /// Admission score when one was stamped or considered.
    pub admission_score: Option<f64>,
    /// EMA state after a continuous observe.
    pub state: Option<f64>,
    /// EMA trend after a continuous observe.
    pub trend: Option<f64>,
    /// Reject reason when `outcome = rejected`.
    pub rejection_reason: Option<String>,
}

/// Sink for persona telemetry.
pub trait PersonaSink: std::fmt::Debug {
    /// Record one event.
    fn emit(&self, event: PersonaEvent);
}

/// In-memory sink for tests and single-process hosts.
#[derive(Debug, Default)]
pub struct PersonaVecSink {
    events: Mutex<Vec<PersonaEvent>>,
}

impl PersonaVecSink {
    /// Snapshot of recorded events.
    ///
    /// # Panics
    /// Only when the sink mutex is poisoned (a bug in this process).
    #[must_use]
    pub fn events(&self) -> Vec<PersonaEvent> {
        self.events.lock().expect("persona sink poisoned").clone()
    }
}

impl PersonaSink for PersonaVecSink {
    fn emit(&self, event: PersonaEvent) {
        self.events
            .lock()
            .expect("persona sink poisoned")
            .push(event);
    }
}

/// Reject empty required strings on the persona tool surface.
fn require_nonempty(value: &str, label: &str) -> Result<(), AmError> {
    if value.is_empty() {
        return Err(AmError::new(
            clio_types::ErrorCode::InvalidArgument,
            format!("{label} is required"),
        ));
    }
    Ok(())
}

fn require_bank_and_timestamp(ctx: &OpContext, timestamp: &str) -> Result<(), AmError> {
    if ctx.bank.as_str().is_empty() {
        return Err(AmError::new(
            clio_types::ErrorCode::MissingBank,
            "bank is required",
        ));
    }
    require_nonempty(timestamp, "timestamp")
}
