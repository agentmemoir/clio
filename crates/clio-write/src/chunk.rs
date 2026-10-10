// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for partitioning session/turn streams into extraction chunks.
//!
//! ## Owns
//! - Turn and chunk shapes
//! - Default short multi-turn chunk size strategy
//!
//! ## Does not own
//! - Extraction, consolidation, or gated writes
//!
//! ## Boundary
//!
//! Changes to how sessions are sliced for parallel extract should normally require changes here.
//! Changes to extraction, consolidation, or gated writes should not require changes here.
//!
//! Keep this module focused on session/turn partitioning.

use serde_json::Value;

/// One conversation or tool-call turn.
#[derive(Debug, Clone, PartialEq)]
pub struct Turn {
    /// Turn prose / tool text (span haystack for this turn).
    pub source_text: String,
    /// Optional structured tool payload.
    pub tool_payload: Option<Value>,
}

impl Turn {
    /// Build a prose-only turn.
    #[must_use]
    pub fn text(source_text: impl Into<String>) -> Self {
        Self {
            source_text: source_text.into(),
            tool_payload: None,
        }
    }
}

/// One extraction chunk produced by partitioning.
#[derive(Debug, Clone, PartialEq)]
pub struct Chunk {
    /// Stable chunk id within the ingest batch (`c0`, `c1`, …).
    pub chunk_id: String,
    /// Concatenated source text for the chunk.
    pub source_text: String,
    /// Optional tool payload (last non-empty in the chunk wins).
    pub tool_payload: Option<Value>,
    /// Original turn indices included in this chunk.
    pub turn_indices: Vec<usize>,
}

/// Default chunk size: short multi-turn windows (MemForest-style).
pub const DEFAULT_MAX_TURNS_PER_CHUNK: usize = 2;

/// Partition turns into extraction chunks of at most `max_turns_per_chunk`.
///
/// Empty `source_text` turns are kept (caller may still fail verify later).
/// `max_turns_per_chunk` of 0 is treated as 1.
#[must_use]
pub fn partition_session(turns: &[Turn], max_turns_per_chunk: usize) -> Vec<Chunk> {
    if turns.is_empty() {
        return Vec::new();
    }
    let cap = max_turns_per_chunk.max(1);
    let mut out = Vec::new();
    let mut start = 0usize;
    while start < turns.len() {
        let end = (start + cap).min(turns.len());
        let slice = &turns[start..end];
        let source_text = slice
            .iter()
            .map(|t| t.source_text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let tool_payload = slice.iter().rev().find_map(|t| t.tool_payload.clone());
        out.push(Chunk {
            chunk_id: format!("c{start}"),
            source_text,
            tool_payload,
            turn_indices: (start..end).collect(),
        });
        start = end;
    }
    out
}

#[cfg(test)]
#[path = "chunk_tests.rs"]
mod chunk_tests;
