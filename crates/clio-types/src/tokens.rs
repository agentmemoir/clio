// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the single token-estimation heuristic shared
//! by every budget surface (persona document, retrieval trimming, compose).
//!
//! ## Owns
//! - `estimate_tokens` (characters / 4, rounded up)
//!
//! ## Does not own
//! - Actual model tokenization (no host LLM tokenizer is loaded)
//! - Packing/truncation policy (per-crate budgeters)
//!
//! ## Boundary
//!
//! Changes to the estimator formula should normally require changes here.
//! Changes to packing order should not require changes here.
//!
//! # Estimator contract
//!
//! `estimate_tokens` approximates an LLM tokenizer as `ceil(chars / 4)` with
//! `chars` counted in Unicode scalar values. It is deliberately cheap and
//! model-independent so storage volume can never force a larger injection
//! Expected error is roughly ±25% on English prose; the budgets that
//! consume it are conservative enough to absorb that. Do not load a host
//! tokenizer on any hot path.

/// Estimate the token count of `text` as `ceil(chars / 4)`.
#[must_use]
pub fn estimate_tokens(text: &str) -> usize {
    let chars = text.chars().count();
    chars.div_ceil(4)
}

#[cfg(test)]
#[path = "tokens_tests.rs"]
mod tokens_tests;
