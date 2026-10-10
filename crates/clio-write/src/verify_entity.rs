// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//
//! # Responsibility
//!
//! This module is responsible for entity/string span matching under NFC + trim.
//!
//! ## Owns
//! - Contiguous substring match for extractive identity strings
//!
//! ## Does not own
//! - Number/date parse rules
//!
//! ## Boundary
//!
//! Changes to entity match / trim rules should normally require changes here.

use serde_json::Value;
use unicode_normalization::UnicodeNormalization;

/// Locate `value` as a contiguous NFC substring of `source` (already NFC).
///
/// Returns Unicode scalar `[start, end)` into `source`.
///
/// # Errors
/// Empty required string, or no exact contiguous match (paraphrase / fuzzy).
pub fn match_entity(source: &str, value: &Value) -> Result<(usize, usize), String> {
    let raw = value
        .as_str()
        .ok_or_else(|| "entity field must be a string".to_owned())?;
    let needle: String = raw.nfc().collect::<String>();
    let needle = needle.trim();
    if needle.is_empty() {
        return Err("empty entity value".into());
    }
    let hay: Vec<char> = source.chars().collect();
    let need: Vec<char> = needle.chars().collect();
    if need.len() > hay.len() {
        return Err("entity not found as contiguous source span".into());
    }
    for start in 0..=(hay.len() - need.len()) {
        if hay[start..start + need.len()] == need[..] {
            return Ok((start, start + need.len()));
        }
    }
    Err("entity not found as contiguous source span".into())
}
