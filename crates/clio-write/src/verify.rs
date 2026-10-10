// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//
//! # Responsibility
//!
//! This module is responsible for span-verify configuration and orchestration.
//!
//! ## Owns
//! - `VerifyConfig`, `VerifyReport`, and `verify_snapshot`
//! - NFC normalization of source text used as the span haystack
//! - The undeclared-leaf scope check: every non-empty snapshot leaf must be a
//!   declared field, else verification fails closed. Unlisted-leaf failures
//!   carry a fixed-shape placeholder, never the caller-supplied key or path
//!
//! ## Does not own
//! - Per-kind matchers (entity / number / date modules)
//! - Extraction or gated admission
//!
//! ## Boundary
//!
//! Changes to verify report shape, field walk order, or leaf scope rules
//! should normally require changes here.
//! Changes to number/date parse details should not require changes here.

use crate::schema::{ExtractiveKind, FieldSpec, json_path};
use crate::verify_date::match_date;
use crate::verify_entity::match_entity;
use crate::verify_number::match_number;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use unicode_normalization::UnicodeNormalization;

/// Verifier knobs (deterministic given identical inputs).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VerifyConfig {
    /// When `None`, ambiguous numeric dates fail closed.
    pub day_first: Option<bool>,
    /// Absolute tolerance for numeric equality (default `0.0`).
    pub number_tolerance: f64,
    /// Prefer `,` grouping / `.` decimal when true (en-US default).
    pub comma_grouping: bool,
}

impl Default for VerifyConfig {
    fn default() -> Self {
        Self {
            day_first: None,
            number_tolerance: 0.0,
            comma_grouping: true,
        }
    }
}

/// Char offsets into the **NFC-normalized** source text as `[start, end)`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpanOffsets {
    /// Inclusive start (Unicode scalar index).
    pub start: usize,
    /// Exclusive end (Unicode scalar index).
    pub end: usize,
    /// Snapshot field path that this span grounds.
    pub field: String,
}

/// One field failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifyFailure {
    /// Field path.
    pub field: String,
    /// Machine-readable reason.
    pub reason: String,
}

/// Result of verifying all extractive fields.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VerifyReport {
    /// True when every required extractive field passed.
    pub ok: bool,
    /// Per-field failures (empty when `ok`).
    pub failures: Vec<VerifyFailure>,
    /// Evidence spans on success (and any partial successes before fail).
    pub spans: Vec<SpanOffsets>,
}

/// NFC-normalize `source` (the span haystack). Callers pass raw source text.
#[must_use]
pub fn nfc(source: &str) -> String {
    source.nfc().collect()
}

/// Verify extractive snapshot fields against `source_text`.
///
/// Spans use Unicode scalar offsets into the NFC form of `source_text`.
///
/// Scope is fail closed: every non-empty snapshot leaf (including nested and
/// list leaves) must be a declared field path, else verification fails. Empty
/// strings, whitespace-only strings, nulls, and empty containers add no leaf.
/// A snapshot that is not a JSON object fails outright.
///
/// An unlisted-leaf failure names the walk position only
/// (`<unlisted leaf #N>`). Snapshot keys are caller-controlled, so the path
/// text is never echoed into a failure, a rejection reason, telemetry, or the
/// extraction retry prompt.
#[must_use]
pub fn verify_snapshot(
    source_text: &str,
    snapshot: &Value,
    fields: &[FieldSpec],
    config: &VerifyConfig,
) -> VerifyReport {
    let source = nfc(source_text);
    if !snapshot.is_object() {
        return VerifyReport {
            ok: false,
            failures: vec![VerifyFailure {
                field: "<snapshot>".into(),
                reason: "snapshot is not an object".into(),
            }],
            spans: Vec::new(),
        };
    }
    let mut failures = Vec::new();
    let mut spans = Vec::new();
    for field in fields {
        match verify_one(&source, snapshot, field, config) {
            Ok(Some(span)) => spans.push(span),
            Ok(None) => {}
            Err(reason) => failures.push(VerifyFailure {
                field: field.path.clone(),
                reason,
            }),
        }
    }
    let declared: HashSet<&str> = fields.iter().map(|f| f.path.as_str()).collect();
    let mut unlisted = 0usize;
    push_unlisted_leaves(snapshot, "", &declared, &mut unlisted, &mut failures);
    VerifyReport {
        ok: failures.is_empty(),
        failures,
        spans,
    }
}

/// Report an unlisted leaf without echoing caller text.
///
/// A snapshot key is caller-controlled, so the offending path may itself carry
/// a secret. The field label is therefore a fixed-shape placeholder that names
/// the walk position only, never the key or the path.
fn unlisted_field(index: usize) -> String {
    format!("<unlisted leaf #{index}>")
}

fn push_unlisted_leaves(
    value: &Value,
    path: &str,
    declared: &HashSet<&str>,
    unlisted: &mut usize,
    failures: &mut Vec<VerifyFailure>,
) {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                let child_path = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                push_unlisted_leaves(child, &child_path, declared, unlisted, failures);
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                let child_path = if path.is_empty() {
                    index.to_string()
                } else {
                    format!("{path}.{index}")
                };
                push_unlisted_leaves(child, &child_path, declared, unlisted, failures);
            }
        }
        leaf => {
            let empty = leaf.is_null() || leaf.as_str().is_some_and(|s| s.trim().is_empty());
            if empty || declared.contains(path) {
                return;
            }
            failures.push(VerifyFailure {
                field: unlisted_field(*unlisted),
                reason: "unlisted snapshot leaf rejected".into(),
            });
            *unlisted += 1;
        }
    }
}

fn verify_one(
    source: &str,
    snapshot: &Value,
    field: &FieldSpec,
    config: &VerifyConfig,
) -> Result<Option<SpanOffsets>, String> {
    let value = json_path(snapshot, &field.path);
    let missing = value.is_none_or(|v| v.is_null() || v.as_str().is_some_and(str::is_empty));
    if missing {
        if field.required {
            return Err("missing required extractive field".into());
        }
        return Ok(None);
    }
    let value = value.expect("checked");
    let (start, end) = match field.kind {
        ExtractiveKind::Entity => match_entity(source, value)?,
        ExtractiveKind::Number => match_number(source, value, config)?,
        ExtractiveKind::Date => match_date(source, value, config)?,
    };
    Ok(Some(SpanOffsets {
        start,
        end,
        field: field.path.clone(),
    }))
}

#[cfg(test)]
#[path = "verify_tests.rs"]
mod verify_tests;

#[cfg(test)]
#[path = "verify_edge_tests.rs"]
mod verify_edge_tests;
