// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `persona_get` read tool: document
//! assembly plus budget truncation (the single source of truth both this crate
//! and the compose channel consume).
//!
//! ## Owns
//! - `persona_get`: read → validate → truncate under the persona budget
//!
//! ## Does not own
//! - Persona SQL (`clio-store::PersonaStore`)
//! - Truncation ranking (`budget`)
//!
//! ## Boundary
//!
//! Changes to read semantics should normally require changes here.
//! Changes to storage layout should not require changes here.

use crate::budget::truncate_document;
use crate::require_bank_and_timestamp;
use clio_store::PersonaStore;
use clio_types::{AmError, OpContext};

/// Inputs for `persona_get`.
pub struct PersonaGetArgs<'a> {
    /// Persona repository.
    pub store: &'a dyn PersonaStore,
    /// Bank / actor context.
    pub ctx: &'a OpContext,
    /// Persona token budget (default [`crate::DEFAULT_PERSONA_TOKEN_BUDGET`]).
    pub budget_tokens: usize,
    /// Event timestamp.
    pub timestamp: &'a str,
}

/// Result of `persona_get`.
#[derive(Debug, Clone, PartialEq)]
pub struct PersonaGetOutcome {
    /// The current, budget-truncated document.
    pub document: clio_types::PersonaDocument,
    /// Keys dropped by the budget (rank order, lowest first).
    pub dropped_keys: Vec<String>,
}

/// Return the current bounded persona document.
///
/// Always-on: never gated by the intent gate; the budget is enforced here by
/// truncation, never by rejection and never by unbounded growth.
///
/// # Errors
/// Missing bank, empty timestamp, or storage/crypto failures.
pub fn persona_get(args: &PersonaGetArgs<'_>) -> Result<PersonaGetOutcome, AmError> {
    require_bank_and_timestamp(args.ctx, args.timestamp)?;
    let full = args.store.get_persona_document(args.ctx.bank.as_str())?;
    full.validate()?;
    let truncation = truncate_document(&full, args.budget_tokens);
    Ok(PersonaGetOutcome {
        document: truncation.document,
        dropped_keys: truncation.dropped_keys,
    })
}

#[cfg(test)]
#[path = "document_compose_wire_tests.rs"]
mod document_compose_wire_tests;
