// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is the crate root of the shared types crate: it declares the
//! type modules and re-exports their public surfaces.
//!
//! ## Owns
//! - Module declarations and the public re-export list
//! - Crate-level attributes (`forbid(unsafe_code)`)
//!
//! ## Does not own
//! - Any type definition itself (each lives in its own module)
//! - I/O of any kind (this crate is pure data and helpers)
//!
//! ## Boundary
//!
//! Changes to the re-export list should normally require changes here.
//! Changes to a type's behaviour should not require changes here.
//!
//! Keep this module focused on assembling the crate's public surface.

#![forbid(unsafe_code)]

pub mod attribute;
pub mod audit_principal;
pub mod belief;
pub mod civil;
pub mod context;
pub mod decay;
pub mod error;
pub mod history;
pub mod item;
pub mod mcp_tools;
pub mod net;
pub mod persona;
pub mod read;
pub mod tokens;
pub mod triple;

pub use attribute::{
    AttributeSchema, Bounds, ContinuousEngineConfig, ContinuousPoint, ContinuousState,
    DEFAULT_ALPHA, DEFAULT_TREND_WINDOW, apply_observation, mean_last_n,
};
pub use audit_principal::{PRINCIPAL_DETAIL_KEY, PrincipalLabel, record_principal};
pub use belief::{BeliefObject, ConfidenceEntry, check_confidence, proposition_key};
pub use civil::{civil_from_days, days_from_civil, days_in_month, is_leap_year};
pub use context::{Actor, Bank, BankDefaultPolicy, OpContext, resolve_bank};
pub use decay::{half_life_factor, ramp_factor};
pub use error::{AmError, ErrorCode, ToolResult};
pub use history::{
    DEFAULT_HISTORY_LIMIT, DEFAULT_LESSON_MAX_TOKENS, DEFAULT_TASK_SIMILARITY_THRESHOLD,
    FailureRecord, HistoryPolicy, MAX_HISTORY_LIMIT, TaskRecord,
};
pub use item::{
    CONTEXT_MAX_BYTES, EpisodicType, EpistemicKind, ItemKind, MemoryItem, SemanticCategory,
    SourceType, validate_context,
};
pub use mcp_tools::{BOUND_READ_TOOLS, BOUND_WRITE_TOOLS, bound_tools, is_bound};
pub use net::{HttpTarget, is_loopback_host, parse_scheme_host, url_scheme};
pub use persona::{
    PERSONA_SOURCES, PersonaDocument, PersonaPreference, PersonaStableEntry,
    validate_persona_source,
};
pub use read::{RetrieveHit, SearchDomain};
pub use tokens::estimate_tokens;
pub use triple::{
    TimeAxis, TripleAddOutcome, TripleEndOutcome, TripleQuery, TripleRecord, UpdateRule,
    ensure_discrete_update_rule, interval_contains,
};

#[cfg(test)]
mod civil_test_support;
