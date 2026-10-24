// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//
//! # Responsibility
//!
//! This module is responsible for the type and range checks of the numeric
//! `config_set` paths (token budgets, limits, thresholds, dimensions, and the
//! reranking batch cap).
//!
//! ## Owns
//! - Every numeric range rule and its rejection message
//! - The upper bounds those rules advertise, including the reranking cap
//!
//! ## Does not own
//! - The allowlist of settable paths and the value dispatcher (see `validate`)
//! - Endpoint safety checks and scope parsing (see `validate`)
//!
//! ## Boundary
//!
//! Changes to an accepted numeric range should normally require changes here.
//! Changes to which paths may be set at all should not require changes here.
//!
//! Keep this module focused on numeric value validation.

use super::validate::MAX_EMBED_DIMS;
use crate::error::{AmError, ErrorCode};
use serde_json::Value;

/// Highest documents-per-request cap accepted for `rerank.max_batch`.
///
/// 1000 is the documented per-request document limit of the hosted reranking
/// services. The bound exists because an accepted value larger than a service
/// will take rebuilds the single oversized request the cap exists to avoid.
pub(crate) const MAX_RERANK_BATCH: u64 = 1_000;

/// Type/range checks for the numeric `config_set` paths.
///
/// One entry point, called by the set-value dispatcher for every path in this
/// module's namespace. A path this module does not own is left alone, so the
/// dispatcher's own arms answer for it.
///
/// # Errors
/// Invalid types or out-of-range values.
pub(crate) fn validate_numeric_value(path: &str, value: &Value) -> Result<(), AmError> {
    match path {
        "injection.budget_tokens" => {
            let Some(n) = value.as_u64() else {
                return Err(AmError::new(
                    ErrorCode::InvalidType,
                    "injection.budget_tokens must be a non-negative integer",
                ));
            };
            if n == 0 || n > 100_000 {
                return Err(AmError::new(
                    ErrorCode::OutOfRange,
                    "injection.budget_tokens must be in 1..=100000",
                ));
            }
        }
        "history.lesson_max_tokens" | "history.default_limit" | "history.max_limit" => {
            let Some(n) = value.as_u64() else {
                return Err(AmError::new(
                    ErrorCode::InvalidType,
                    format!("{path} must be a positive integer"),
                ));
            };
            if n == 0 || n > 100_000 {
                return Err(AmError::new(
                    ErrorCode::OutOfRange,
                    format!("{path} must be in 1..=100000"),
                ));
            }
        }
        "history.similarity_threshold" => {
            let Some(n) = value.as_f64() else {
                return Err(AmError::new(
                    ErrorCode::InvalidType,
                    "history.similarity_threshold must be a number",
                ));
            };
            if !(0.0..=1.0).contains(&n) {
                return Err(AmError::new(
                    ErrorCode::OutOfRange,
                    "history.similarity_threshold must be in 0.0..=1.0",
                ));
            }
        }
        "extract.temperature" => {
            let Some(n) = value.as_f64() else {
                return Err(AmError::new(
                    ErrorCode::InvalidType,
                    "extract.temperature must be a number",
                ));
            };
            if !(0.0..=2.0).contains(&n) {
                return Err(AmError::new(
                    ErrorCode::OutOfRange,
                    "extract.temperature must be in 0.0..=2.0",
                ));
            }
        }
        "embed.dims" => {
            let Some(n) = value.as_u64() else {
                return Err(AmError::new(
                    ErrorCode::InvalidType,
                    "embed.dims must be a positive integer",
                ));
            };
            if n == 0 || n > MAX_EMBED_DIMS {
                return Err(AmError::new(
                    ErrorCode::OutOfRange,
                    format!("embed.dims must be in 1..={MAX_EMBED_DIMS} (storage ceiling)"),
                ));
            }
        }
        "rerank.max_batch" => validate_rerank_max_batch(value)?,
        _ => {}
    }
    Ok(())
}

/// Reject a document cap the TEI reranker cannot honour in one request.
///
/// The cap belongs to the TEI adapter alone: it chunks requests by this value,
/// while the Cohere adapter sends the shortlist in one call and ignores it. A
/// value above the service's own client batch ceiling is refused rather than
/// accepted and then answered with an oversized-request failure.
///
/// # Errors
/// A non-integer, zero, or over-ceiling value.
fn validate_rerank_max_batch(value: &Value) -> Result<(), AmError> {
    // One message for both rejections: an operator who typed a bad cap needs the
    // accepted shape and the two ceilings it has to fit under, whatever they typed.
    let message = || {
        format!(
            "rerank.max_batch must be a positive integer no greater than {MAX_RERANK_BATCH} \
             (the TEI reranker's documents-per-request cap; the Cohere provider ignores it), and \
             the value must not exceed the rerank service's `--max-client-batch-size` ceiling"
        )
    };
    let Some(n) = value.as_u64() else {
        return Err(AmError::new(ErrorCode::InvalidType, message()));
    };
    // A request carries at least one document, so `0` is not a cap.
    if n == 0 {
        return Err(AmError::new(ErrorCode::InvalidType, message()));
    }
    if n > MAX_RERANK_BATCH {
        return Err(AmError::new(ErrorCode::OutOfRange, message()));
    }
    Ok(())
}

#[cfg(test)]
#[path = "validate_set_tests.rs"]
mod tests;
