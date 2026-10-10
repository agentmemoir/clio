// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the typed weight groups and the shared
//! normalize-or-reject primitive used by ranking/admission/hygiene knobs.
//!
//! ## Owns
//! - `RetrievalWeights`, `AdmissionWeights`, `IntentGateThresholds`
//! - [`normalize_weights`]: non-negative renormalize-or-reject
//!
//! ## Does not own
//! - The ranking env surface and patch application (see `ranking`)
//! - Hygiene weight defaults (see `hygiene_knobs`)
//!
//! ## Boundary
//!
//! Changes to weight-group schemas or the normalization rule should normally
//! require changes here. Changes to patch merging should not require changes
//! here.
//!
//! Keep this module focused on weight groups and normalization.

use crate::error::{AmError, ErrorCode};
use serde::{Deserialize, Serialize};

/// Hybrid retrieval blend weights; MUST sum to 1.0 when applied.
///
/// A key this group does not declare is rejected rather than ignored, so a
/// weight retired from the blend cannot ride along inside a patch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetrievalWeights {
    pub dense: f64,
    pub lexical: f64,
    pub importance: f64,
}

impl Default for RetrievalWeights {
    fn default() -> Self {
        // One expression, so the group keeps summing to 1.0 if a weight returns.
        let each = 1.0 / 3.0;
        Self {
            dense: each,
            lexical: each,
            importance: each,
        }
    }
}

impl RetrievalWeights {
    pub(crate) fn as_slice(&self) -> [f64; 3] {
        [self.dense, self.lexical, self.importance]
    }

    pub(crate) fn from_slice(v: [f64; 3]) -> Self {
        Self {
            dense: v[0],
            lexical: v[1],
            importance: v[2],
        }
    }
}

/// Four-factor admission weights `w1…w4` MUST sum to 1.0 when applied.
///
/// A key this group does not declare is rejected rather than ignored, so a
/// weight retired from the blend cannot ride along inside a patch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdmissionWeights {
    pub w1: f64,
    pub w2: f64,
    pub w3: f64,
    pub w4: f64,
}

impl Default for AdmissionWeights {
    fn default() -> Self {
        // Four factors, one quarter each, so the group sums to 1.0 exactly.
        Self {
            w1: 0.25,
            w2: 0.25,
            w3: 0.25,
            w4: 0.25,
        }
    }
}

impl AdmissionWeights {
    pub(crate) fn as_slice(&self) -> [f64; 4] {
        [self.w1, self.w2, self.w3, self.w4]
    }

    pub(crate) fn from_slice(v: [f64; 4]) -> Self {
        Self {
            w1: v[0],
            w2: v[1],
            w3: v[2],
            w4: v[3],
        }
    }
}

/// Optional intent-gate thresholds (deployment-tunable; unused until retrieval is implemented).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct IntentGateThresholds {
    /// Minimum model confidence to skip retrieval when gate says "not needed".
    pub skip_confidence: Option<f64>,
}

/// Documented rule: non-negative weights with sum &gt; 0 are **renormalized** to 1.0;
/// negative weights or a zero sum are **rejected**.
///
/// # Errors
/// Returns [`ErrorCode::OutOfRange`] when any weight is negative/non-finite or the sum is zero.
pub fn normalize_weights(weights: &[f64]) -> Result<Vec<f64>, AmError> {
    if weights.iter().any(|w| !w.is_finite() || *w < 0.0) {
        return Err(AmError::new(
            ErrorCode::OutOfRange,
            "ranking weights must be finite and non-negative",
        ));
    }
    let sum: f64 = weights.iter().sum();
    if sum <= 0.0 {
        return Err(AmError::new(
            ErrorCode::OutOfRange,
            "ranking weights must sum to a positive value (got 0)",
        ));
    }
    Ok(weights.iter().map(|w| w / sum).collect())
}

#[cfg(test)]
#[path = "weight_groups_tests.rs"]
mod weight_groups_tests;
