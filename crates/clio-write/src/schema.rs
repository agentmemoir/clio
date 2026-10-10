// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//
//! # Responsibility
//!
//! This module is responsible for extractive field specs and JSON path reads.
//!
//! ## Owns
//! - `FieldSpec` / `ExtractiveKind` and the default event field list
//! - Dot-path lookup into snapshot JSON
//!
//! ## Does not own
//! - Span matching or extraction backends
//!
//! ## Boundary
//!
//! Changes to which fields require span verify should normally require changes here.
//! Changes to match rules should not require changes here.

use serde_json::Value;

/// How a snapshot field must be grounded in source text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtractiveKind {
    /// Contiguous NFC substring (entity / identity string).
    Entity,
    /// Locatable parseable number span.
    Number,
    /// Locatable parseable date/time span.
    Date,
}

/// One extractive field inside a candidate snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldSpec {
    /// Dot path into the snapshot object (e.g. `amount`).
    pub path: String,
    /// Match rule for this field.
    pub kind: ExtractiveKind,
    /// Missing or empty value fails verification when true.
    pub required: bool,
}

impl FieldSpec {
    /// Build a field spec.
    #[must_use]
    pub fn new(path: impl Into<String>, kind: ExtractiveKind, required: bool) -> Self {
        Self {
            path: path.into(),
            kind,
            required,
        }
    }
}

/// Default memory-event extractive fields for fixture / NuExtract templates.
#[must_use]
pub fn default_event_fields() -> Vec<FieldSpec> {
    vec![
        FieldSpec::new("entity", ExtractiveKind::Entity, true),
        FieldSpec::new("amount", ExtractiveKind::Number, false),
        FieldSpec::new("date", ExtractiveKind::Date, false),
    ]
}

/// Read a dotted path from a JSON object; `None` if missing.
#[must_use]
pub fn json_path<'a>(root: &'a Value, path: &str) -> Option<&'a Value> {
    let mut cur = root;
    for part in path.split('.').filter(|p| !p.is_empty()) {
        cur = cur.as_object()?.get(part)?;
    }
    Some(cur)
}

#[cfg(test)]
#[path = "schema_tests.rs"]
mod schema_tests;
