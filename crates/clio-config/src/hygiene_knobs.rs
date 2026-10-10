// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for hygiene noise-score weights in the ranking
//! env (appendix): the `HygieneWeights` struct, defaults, and the
//! staleness half-life knob.
//!
//! ## Owns
//! - `HygieneWeights` structure, defaults, and slice conversion
//! - `hygiene_stale_half_life_days` default value
//!
//! ## Does not own
//! - Score composition (`clio-hygiene`)
//! - Retrieval/admission weight groups (see `ranking`)
//!
//! ## Boundary
//!
//! Changes to hygiene knob schema should normally require changes here.
//! Changes to hygiene scoring math should not require changes here.
//!
//! Keep this module focused on hygiene ranking-adjacent knobs.

use serde::{Deserialize, Serialize};

/// Hygiene noise-score weights (appendix); MUST sum to 1.0 when applied.
///
/// Distinct from admission weights: these rank operational noise, not admission.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HygieneWeights {
    /// Credential / token / DEK-looking material detected (default 0.35).
    pub secret: f64,
    /// Terminal spam, stack traces, heartbeats, command dumps (default 0.25).
    pub pattern: f64,
    /// Near-duplicate of a higher-utility retained item (default 0.15).
    pub dup: f64,
    /// Inverse coarse utility proxy (default 0.15).
    pub utility: f64,
    /// Age without recent retrieval / reinforce (default 0.10).
    pub staleness: f64,
}

impl Default for HygieneWeights {
    fn default() -> Self {
        Self {
            secret: 0.35,
            pattern: 0.25,
            dup: 0.15,
            utility: 0.15,
            staleness: 0.10,
        }
    }
}

impl HygieneWeights {
    pub(crate) fn as_slice(&self) -> [f64; 5] {
        [
            self.secret,
            self.pattern,
            self.dup,
            self.utility,
            self.staleness,
        ]
    }

    pub(crate) fn from_slice(v: [f64; 5]) -> Self {
        Self {
            secret: v[0],
            pattern: v[1],
            dup: v[2],
            utility: v[3],
            staleness: v[4],
        }
    }
}

/// Default staleness half-life (days) for hygiene noise scoring.
pub const DEFAULT_HYGIENE_STALE_HALF_LIFE_DAYS: f64 = 90.0;

/// Returns the default staleness half-life (days) for hygiene noise scoring.
#[must_use]
pub const fn default_hygiene_stale_half_life() -> f64 {
    DEFAULT_HYGIENE_STALE_HALF_LIFE_DAYS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hygiene_weight_defaults_match_appendix() {
        let w = HygieneWeights::default();
        let sum: f64 = w.as_slice().iter().sum();
        assert!((sum - 1.0).abs() < 1e-9, "appendix weights sum to 1.0");
        assert!((w.secret - 0.35).abs() < 1e-9);
        assert!((w.pattern - 0.25).abs() < 1e-9);
        assert!((w.dup - 0.15).abs() < 1e-9);
        assert!((w.utility - 0.15).abs() < 1e-9);
        assert!((w.staleness - 0.10).abs() < 1e-9);
    }

    #[test]
    fn stale_half_life_default_is_ninety_days() {
        assert!((default_hygiene_stale_half_life() - 90.0).abs() < 1e-9);
        assert!((DEFAULT_HYGIENE_STALE_HALF_LIFE_DAYS - 90.0).abs() < 1e-9);
    }
}
