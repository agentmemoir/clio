// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the history threshold knobs surfaced through
//! configuration: the failure lesson cap, similar-task threshold, and bounded
//! read sizes.
//!
//! ## Owns
//! - `HistoryKnobs` defaults, `from_config` extraction, and `to_policy`
//! - Conversion into an `clio_types::HistoryPolicy`
//!
//! ## Does not own
//! - Enforcing caps or running similarity (see `clio-history` / `clio-types`)
//! - The config overlay merge or tool dispatch (see `config`)
//!
//! ## Boundary
//!
//! Changes to the history knob shape or defaults should normally require
//! changes here. Changes to config merge/dispatch should not.

use crate::error::AmError;
use clio_types::{
    DEFAULT_HISTORY_LIMIT, DEFAULT_LESSON_MAX_TOKENS, DEFAULT_TASK_SIMILARITY_THRESHOLD,
    HistoryPolicy, MAX_HISTORY_LIMIT,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Deployment-tunable history thresholds, seeded from the `clio-types` defaults.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryKnobs {
    /// Hard cap on a failure `lesson`, in estimated tokens.
    pub lesson_max_tokens: usize,
    /// Minimum lexical similarity for a similar-task match, in `0.0..=1.0`.
    pub similarity_threshold: f64,
    /// Rows returned when a caller omits `limit`.
    pub default_limit: usize,
    /// Hard ceiling applied to every history read.
    pub max_limit: usize,
}

impl Default for HistoryKnobs {
    fn default() -> Self {
        Self {
            lesson_max_tokens: DEFAULT_LESSON_MAX_TOKENS,
            similarity_threshold: DEFAULT_TASK_SIMILARITY_THRESHOLD,
            default_limit: DEFAULT_HISTORY_LIMIT,
            max_limit: MAX_HISTORY_LIMIT,
        }
    }
}

impl HistoryKnobs {
    /// Read the `history.*` section of an effective-config document, falling
    /// back to the defaults for any missing or mistyped key.
    #[must_use]
    pub fn from_config(raw: &Value) -> Self {
        let defaults = Self::default();
        let get_usize = |key: &str, fallback: usize| {
            raw.pointer(&format!("/history/{key}"))
                .and_then(Value::as_u64)
                .and_then(|n| usize::try_from(n).ok())
                .unwrap_or(fallback)
        };
        Self {
            lesson_max_tokens: get_usize("lesson_max_tokens", defaults.lesson_max_tokens),
            similarity_threshold: raw
                .pointer("/history/similarity_threshold")
                .and_then(Value::as_f64)
                .unwrap_or(defaults.similarity_threshold),
            default_limit: get_usize("default_limit", defaults.default_limit),
            max_limit: get_usize("max_limit", defaults.max_limit),
        }
    }

    /// Build the validated runtime `HistoryPolicy` from these knobs.
    ///
    /// # Errors
    /// Zero cap/limit, `default_limit > max_limit`, or a similarity threshold
    /// outside `0.0..=1.0`.
    pub fn to_policy(&self) -> Result<HistoryPolicy, AmError> {
        HistoryPolicy::new(
            self.lesson_max_tokens,
            self.similarity_threshold,
            self.default_limit,
            self.max_limit,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorCode;
    use serde_json::json;

    fn approx(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-9, "{a} != {b}");
    }

    #[test]
    fn defaults_match_types_and_build_policy() {
        let knobs = HistoryKnobs::default();
        assert_eq!(knobs.lesson_max_tokens, DEFAULT_LESSON_MAX_TOKENS);
        approx(
            knobs.similarity_threshold,
            DEFAULT_TASK_SIMILARITY_THRESHOLD,
        );
        assert_eq!(knobs.default_limit, DEFAULT_HISTORY_LIMIT);
        assert_eq!(knobs.max_limit, MAX_HISTORY_LIMIT);
        let policy = knobs.to_policy().expect("valid defaults");
        assert_eq!(policy.max_limit, MAX_HISTORY_LIMIT);
    }

    #[test]
    fn from_config_reads_overrides_and_falls_back() {
        let raw = json!({"history": {"lesson_max_tokens": 50, "default_limit": 3}});
        let knobs = HistoryKnobs::from_config(&raw);
        assert_eq!(knobs.lesson_max_tokens, 50);
        assert_eq!(knobs.default_limit, 3);
        approx(
            knobs.similarity_threshold,
            DEFAULT_TASK_SIMILARITY_THRESHOLD,
        );
        assert_eq!(knobs.max_limit, MAX_HISTORY_LIMIT);

        // Partial / null documents fall back without panicking.
        let knobs = HistoryKnobs::from_config(&json!({"history": {"similarity_threshold": 0.9}}));
        assert_eq!(knobs.lesson_max_tokens, DEFAULT_LESSON_MAX_TOKENS);
        approx(knobs.similarity_threshold, 0.9);
        let knobs = HistoryKnobs::from_config(&Value::Null);
        assert_eq!(knobs, HistoryKnobs::default());
    }

    #[test]
    fn invalid_knobs_are_rejected() {
        let knobs = HistoryKnobs {
            default_limit: 0,
            ..HistoryKnobs::default()
        };
        assert_eq!(knobs.to_policy().unwrap_err().code, ErrorCode::OutOfRange);
        let knobs = HistoryKnobs {
            similarity_threshold: 2.0,
            ..HistoryKnobs::default()
        };
        assert_eq!(knobs.to_policy().unwrap_err().code, ErrorCode::OutOfRange);
    }
}
