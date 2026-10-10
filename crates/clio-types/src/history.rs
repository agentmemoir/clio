// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the task/failure history shapes and the
//! bounded-read policy shared by the history tools.
//!
//! ## Owns
//! - `TaskRecord` / `FailureRecord` shapes and structural validation
//! - `HistoryPolicy`: lesson token cap, similarity threshold, bounded read limits
//!
//! ## Does not own
//! - Persistence, encryption, admission scoring, or similarity math
//!
//! ## Boundary
//!
//! Changes to history field shapes and the cap/limit policy should normally
//! require changes here. Changes to history SQL or similarity algorithms should
//! not require changes here.
//!
//! Keep this module focused on history shapes and read bounds.

use crate::error::{AmError, ErrorCode};
use crate::item::{EpistemicKind, validate_context};
use crate::tokens::estimate_tokens;
use serde::{Deserialize, Serialize};

/// Default hard cap on a `FailureRecord` `lesson`, in estimated tokens.
pub const DEFAULT_LESSON_MAX_TOKENS: usize = 120;
/// Default lexical similarity threshold for "similar task" matching.
pub const DEFAULT_TASK_SIMILARITY_THRESHOLD: f64 = 0.5;
/// Default number of history rows returned when a caller omits `limit`.
pub const DEFAULT_HISTORY_LIMIT: usize = 20;
/// Hard ceiling on any history read, regardless of a caller's `limit`.
pub const MAX_HISTORY_LIMIT: usize = 200;

/// Selection policy for history writes and bounded reads.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryPolicy {
    /// Hard cap on a failure `lesson`, in estimated tokens (reject over cap).
    pub lesson_max_tokens: usize,
    /// Minimum lexical similarity for a similar-task match, in `0.0..=1.0`.
    pub similarity_threshold: f64,
    /// Rows returned when the caller omits `limit`.
    pub default_limit: usize,
    /// Hard ceiling applied to every read, even when the caller asks for more.
    pub max_limit: usize,
}

impl Default for HistoryPolicy {
    fn default() -> Self {
        Self {
            lesson_max_tokens: DEFAULT_LESSON_MAX_TOKENS,
            similarity_threshold: DEFAULT_TASK_SIMILARITY_THRESHOLD,
            default_limit: DEFAULT_HISTORY_LIMIT,
            max_limit: MAX_HISTORY_LIMIT,
        }
    }
}

impl HistoryPolicy {
    /// Build a validated policy.
    ///
    /// # Errors
    /// Zero lesson cap / max limit, `default_limit > max_limit`, or a similarity
    /// threshold outside `0.0..=1.0`.
    pub fn new(
        lesson_max_tokens: usize,
        similarity_threshold: f64,
        default_limit: usize,
        max_limit: usize,
    ) -> Result<Self, AmError> {
        if lesson_max_tokens == 0 {
            return Err(AmError::new(
                ErrorCode::OutOfRange,
                "lesson_max_tokens must be positive",
            ));
        }
        if !(0.0..=1.0).contains(&similarity_threshold) {
            return Err(AmError::new(
                ErrorCode::OutOfRange,
                "similarity_threshold must be in 0.0..=1.0",
            ));
        }
        if default_limit == 0 || max_limit == 0 || default_limit > max_limit {
            return Err(AmError::new(
                ErrorCode::OutOfRange,
                "history limits must satisfy 0 < default_limit <= max_limit",
            ));
        }
        Ok(Self {
            lesson_max_tokens,
            similarity_threshold,
            default_limit,
            max_limit,
        })
    }

    /// Resolve a caller `limit` to a bounded row count.
    ///
    /// `None` → `default_limit`; `Some(0)` is rejected; larger values are
    /// silently capped at `max_limit` so no read can dump an unbounded bank
    /// history.
    ///
    /// # Errors
    /// Zero `limit`.
    pub fn resolve_limit(&self, limit: Option<usize>) -> Result<usize, AmError> {
        match limit {
            None => Ok(self.default_limit),
            Some(0) => Err(AmError::new(
                ErrorCode::OutOfRange,
                "limit must be positive",
            )),
            Some(n) => Ok(n.min(self.max_limit)),
        }
    }

    /// Enforce the hard `lesson` cap; returns the stored token count.
    ///
    /// Over-cap lessons are rejected with a structured `out_of_range` error
    /// (documented choice: reject, never store an unbounded lesson).
    ///
    /// # Errors
    /// Lesson over the configured token cap.
    pub fn check_lesson(&self, lesson: &str) -> Result<usize, AmError> {
        let tokens = estimate_tokens(lesson);
        if tokens > self.lesson_max_tokens {
            return Err(AmError::new(
                ErrorCode::OutOfRange,
                format!(
                    "lesson is {tokens} tokens, over the {} token cap",
                    self.lesson_max_tokens
                ),
            ));
        }
        Ok(tokens)
    }
}

/// One task-history version: a task definition, its steps, and status.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskRecord {
    /// Stable record id (one row per version).
    pub id: String,
    /// Bank isolation key.
    pub bank_id: String,
    /// Backing episodic `task` carrier item id.
    pub item_id: String,
    /// Caller task identifier (not globally unique; versioned per bank).
    pub task_id: String,
    /// Data-subject id for DEK selection.
    pub subject_id: String,
    /// Monotonic version within `(bank, task_id)`, starting at 1.
    pub version: i64,
    /// Task definition (plaintext in memory; encrypted at rest).
    pub definition: String,
    /// Ordered steps taken.
    pub steps: Vec<String>,
    /// Free-form status label.
    pub status: String,
    /// Optional evidence reference (turn id / tool call id).
    pub evidence_ref: Option<String>,
    /// Optional bounded task context.
    ///
    /// Sealed inside the backing carrier item's dual-content ciphertext; the
    /// key is omitted from serialization when absent (no empty-string sentinel).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    /// Admission score stamped by the gated write.
    pub admission_score: f64,
    /// Created timestamp.
    pub created_at: String,
}

impl TaskRecord {
    /// Structural well-formedness (not admission scoring).
    ///
    /// # Errors
    /// Empty ids/definition/status, an out-of-range admission score, or a
    /// `context` that is empty or over the published byte maximum.
    pub fn validate(&self) -> Result<(), AmError> {
        if self.id.is_empty()
            || self.bank_id.is_empty()
            || self.item_id.is_empty()
            || self.task_id.is_empty()
            || self.subject_id.is_empty()
        {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "task record id, bank_id, item_id, task_id, and subject_id are required",
            ));
        }
        if self.version < 1 {
            return Err(AmError::new(
                ErrorCode::OutOfRange,
                "task record version must be >= 1",
            ));
        }
        if self.definition.is_empty() {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "task record definition is required",
            ));
        }
        if self.status.is_empty() {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "task record status is required",
            ));
        }
        check_score(self.admission_score, "admission_score")?;
        if let Some(context) = &self.context {
            validate_context(context)?;
        }
        Ok(())
    }
}

/// Structured failure memory: what failed and the capped lesson.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FailureRecord {
    /// Stable record id.
    pub id: String,
    /// Bank isolation key.
    pub bank_id: String,
    /// Backing episodic `failure` carrier item id.
    pub item_id: String,
    /// Task this failure belongs to.
    pub task_id: String,
    /// Data-subject id for DEK selection.
    pub subject_id: String,
    /// 1-based attempt number within `(bank, task_id)`.
    pub attempt_n: i64,
    /// What went wrong.
    ///
    /// Deliberately not length-capped: this is the faithful factual record and
    /// must not be truncated. Only `lesson` (the natural-language reinforcement
    /// surfaced into later context) is token-capped.
    pub what_failed: String,
    /// Length-capped natural-language lesson (Rej).
    pub lesson: String,
    /// Estimated token count of `lesson` at write time.
    pub lesson_tokens: i64,
    /// Optional evidence reference (trace id / tool call id).
    pub evidence_ref: Option<String>,
    /// Optional bounded failure context.
    ///
    /// Sealed inside the backing carrier item's dual-content ciphertext; the
    /// key is omitted from serialization when absent (no empty-string sentinel).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    /// Admission score stamped by the gated write.
    pub admission_score: f64,
    /// Failure records are `epistemic_kind = belief`.
    pub epistemic_kind: EpistemicKind,
    /// Created timestamp.
    pub created_at: String,
}

impl FailureRecord {
    /// Structural well-formedness (not admission scoring or the lesson cap).
    ///
    /// # Errors
    /// Empty ids/`what_failed`, `attempt_n < 1`, a non-belief kind, an
    /// out-of-range admission score, or a `context` that is empty or over the
    /// published byte maximum.
    pub fn validate(&self) -> Result<(), AmError> {
        if self.id.is_empty()
            || self.bank_id.is_empty()
            || self.item_id.is_empty()
            || self.task_id.is_empty()
            || self.subject_id.is_empty()
        {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "failure record id, bank_id, item_id, task_id, and subject_id are required",
            ));
        }
        if self.attempt_n < 1 {
            return Err(AmError::new(
                ErrorCode::OutOfRange,
                "failure record attempt_n must be >= 1",
            ));
        }
        if self.what_failed.is_empty() {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "failure record what_failed is required",
            ));
        }
        if self.epistemic_kind != EpistemicKind::Belief {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "failure record epistemic_kind must be belief",
            ));
        }
        check_score(self.admission_score, "admission_score")?;
        if let Some(context) = &self.context {
            validate_context(context)?;
        }
        Ok(())
    }
}

fn check_score(score: f64, label: &str) -> Result<(), AmError> {
    if score.is_finite() && (0.0..=1.0).contains(&score) {
        return Ok(());
    }
    Err(AmError::new(
        ErrorCode::OutOfRange,
        format!("{label} must be in 0.0..=1.0"),
    ))
}

#[cfg(test)]
#[path = "history_tests.rs"]
mod history_tests;
