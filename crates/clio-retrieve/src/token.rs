// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module re-exports the single token-estimation heuristic shared by every
//! budget surface (defined once in `clio-types::tokens` so the persona document
//! and compose account tokens identically).
//!
//! ## Owns
//! - The `estimate_tokens` re-export for this crate's budget surfaces
//!
//! ## Does not own
//! - The estimator formula (`clio-types::tokens`)
//!
//! ## Boundary
//!
//! Changes to the formula belong in `clio-types::tokens`; callers here keep using
//! the same function name.

pub use clio_types::tokens::estimate_tokens;
