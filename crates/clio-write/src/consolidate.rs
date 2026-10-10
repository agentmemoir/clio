// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for deterministic near-duplicate consolidation.
//!
//! ## Owns
//! - Surface-form normalization and fingerprinting
//! - Discrete-key contradiction split (subject+predicate / attribute key)
//! - Provenance union and confidence aggregation (max)
//!
//! ## Does not own
//! - Embedding/LLM equivalence (flagged off; not sole merge authority)
//! - Bi-temporal supersession
//!
//! ## Boundary
//!
//! Changes to merge / no-merge rules should normally require changes here.
//! Changes to admission gating or triple supersession should not require changes here.
//!
//! Keep this module focused on deterministic near-duplicate consolidation.

use crate::extract::ExtractCandidate;
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use unicode_normalization::UnicodeNormalization;

/// Verified extract candidate awaiting consolidation.
#[derive(Debug, Clone, PartialEq)]
pub struct VerifiedCandidate {
    /// Source chunk id.
    pub chunk_id: String,
    /// Span haystack for this candidate.
    pub source_text: String,
    /// Verified extract output.
    pub candidate: ExtractCandidate,
    /// Optional confidence (default 1.0 when absent).
    pub confidence: f64,
}

/// Stable canonical write unit after merge / split.
#[derive(Debug, Clone, PartialEq)]
pub struct CanonicalUnit {
    /// Canonical snapshot (authoritative when present). Merge keeps the first
    /// contributing candidate's snapshot unmodified and only unions provenance
    /// and source text, so span verification stays grounded in the original sources.
    pub snapshot: Option<Value>,
    /// Merged gist (non-authoritative).
    pub gist: Option<String>,
    /// Union of source_ref provenance.
    pub source_refs: Vec<String>,
    /// Chunk ids that contributed.
    pub chunk_ids: Vec<String>,
    /// Source texts joined for re-verify after merge rewrite.
    pub source_text: String,
    /// Aggregated confidence (max of members).
    pub confidence: f64,
    /// Discrete key when SPO / attribute shaped.
    pub discrete_key: Option<String>,
    /// True when this unit was kept separate due to a contradictory object.
    pub contradiction_split: bool,
    /// Note for a later triple supersession handoff.
    pub triple_handoff_note: Option<String>,
}

/// Consolidation telemetry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ConsolidateStats {
    /// How many input candidates were merged into an existing unit.
    pub merges: u32,
    /// How many contradiction splits were emitted.
    pub contradiction_splits: u32,
    /// How many canonical units were produced.
    pub units: u32,
}

/// Result of consolidation.
#[derive(Debug, Clone, PartialEq)]
pub struct ConsolidateResult {
    /// Surviving canonical units (order: first-seen fingerprint / key).
    pub units: Vec<CanonicalUnit>,
    /// Merge counters.
    pub stats: ConsolidateStats,
}

/// NFC + lowercase + collapse whitespace for surface forms.
#[must_use]
pub fn normalize_surface(s: &str) -> String {
    let nfc: String = s.nfc().collect();
    nfc.split_whitespace()
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join(" ")
}

fn normalize_value(v: &Value) -> Value {
    match v {
        Value::String(s) => Value::String(normalize_surface(s)),
        Value::Array(arr) => Value::Array(arr.iter().map(normalize_value).collect()),
        Value::Object(map) => {
            let mut out = Map::new();
            let mut keys: Vec<_> = map.keys().cloned().collect();
            keys.sort();
            for k in keys {
                if let Some(val) = map.get(&k) {
                    out.insert(k, normalize_value(val));
                }
            }
            Value::Object(out)
        }
        other => other.clone(),
    }
}

/// Deterministic fingerprint over the normalized snapshot (or gist fallback).
#[must_use]
pub fn fingerprint(candidate: &ExtractCandidate) -> u64 {
    let payload = if let Some(snap) = &candidate.snapshot {
        normalize_value(snap)
    } else {
        Value::String(normalize_surface(candidate.gist.as_deref().unwrap_or("")))
    };
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    serde_json::to_string(&payload)
        .unwrap_or_default()
        .hash(&mut hasher);
    hasher.finish()
}

fn str_field(snap: &Value, keys: &[&str]) -> Option<String> {
    let obj = snap.as_object()?;
    for k in keys {
        if let Some(Value::String(s)) = obj.get(*k) {
            let n = normalize_surface(s);
            if !n.is_empty() {
                return Some(n);
            }
        }
    }
    None
}

/// Discrete key for SPO-shaped or explicit attribute snapshots.
///
/// Forms recognized:
/// - `subject`+`predicate` (aliases `s`/`p`)
/// - `attribute` / `attr` / `key` as a single discrete attribute key
#[must_use]
pub fn discrete_key(snapshot: &Value) -> Option<String> {
    if let (Some(sub), Some(pred)) = (
        str_field(snapshot, &["subject", "s"]),
        str_field(snapshot, &["predicate", "p", "rel", "relation"]),
    ) {
        return Some(format!("{sub}|{pred}"));
    }
    str_field(snapshot, &["attribute", "attr", "key"]).map(|a| format!("attr:{a}"))
}

fn discrete_object(snapshot: &Value) -> Option<String> {
    str_field(snapshot, &["object", "o", "value", "val"])
}

fn confidence_of(c: &VerifiedCandidate) -> f64 {
    if (0.0..=1.0).contains(&c.confidence) {
        c.confidence
    } else {
        1.0
    }
}

fn unit_from(
    c: &VerifiedCandidate,
    contradiction_split: bool,
    note: Option<String>,
) -> CanonicalUnit {
    CanonicalUnit {
        snapshot: c.candidate.snapshot.clone(),
        gist: c.candidate.gist.clone(),
        source_refs: vec![c.candidate.source_ref.clone()],
        chunk_ids: vec![c.chunk_id.clone()],
        source_text: c.source_text.clone(),
        confidence: confidence_of(c),
        discrete_key: c.candidate.snapshot.as_ref().and_then(discrete_key),
        contradiction_split,
        triple_handoff_note: note,
    }
}

fn merge_into(unit: &mut CanonicalUnit, c: &VerifiedCandidate) {
    if unit.gist.as_ref().map_or(0, String::len) < c.candidate.gist.as_ref().map_or(0, String::len)
    {
        unit.gist.clone_from(&c.candidate.gist);
    }
    if !unit.source_refs.contains(&c.candidate.source_ref) {
        unit.source_refs.push(c.candidate.source_ref.clone());
    }
    if !unit.chunk_ids.contains(&c.chunk_id) {
        unit.chunk_ids.push(c.chunk_id.clone());
    }
    if !unit.source_text.contains(&c.source_text) {
        unit.source_text = format!("{}\n{}", unit.source_text, c.source_text);
    }
    unit.confidence = unit.confidence.max(confidence_of(c));
}

/// Merge near-duplicates by deterministic fingerprint; never blend discrete contradictions.
///
/// Embedding/LLM merge is intentionally absent from the default path.
#[must_use]
pub fn consolidate(candidates: &[VerifiedCandidate]) -> ConsolidateResult {
    let mut units: Vec<CanonicalUnit> = Vec::new();
    // fingerprint → unit index (non-keyed or matching-object keyed)
    let mut by_fp: BTreeMap<u64, usize> = BTreeMap::new();
    // discrete key → (object_norm, unit index)
    let mut by_key: BTreeMap<String, Vec<(String, usize)>> = BTreeMap::new();
    let mut stats = ConsolidateStats::default();

    for c in candidates {
        let fp = fingerprint(&c.candidate);
        let key = c.candidate.snapshot.as_ref().and_then(discrete_key);
        let obj = c
            .candidate
            .snapshot
            .as_ref()
            .and_then(discrete_object)
            .unwrap_or_default();

        if let Some(ref dk) = key {
            let entries = by_key.entry(dk.clone()).or_default();
            if let Some((_, idx)) = entries.iter().find(|(o, _)| o == &obj) {
                merge_into(&mut units[*idx], c);
                stats.merges += 1;
                continue;
            }
            if !entries.is_empty() && !obj.is_empty() {
                // Same discrete key, different object → separate unit + handoff note.
                let note = Some(format!(
                    "discrete_key_contradiction:{dk}; handoff=triple_supersession"
                ));
                let idx = units.len();
                units.push(unit_from(c, true, note));
                entries.push((obj, idx));
                stats.contradiction_splits += 1;
                continue;
            }
            let idx = units.len();
            units.push(unit_from(c, false, None));
            entries.push((obj, idx));
            by_fp.insert(fp, idx);
            continue;
        }

        if let Some(&idx) = by_fp.get(&fp) {
            merge_into(&mut units[idx], c);
            stats.merges += 1;
            continue;
        }
        let idx = units.len();
        units.push(unit_from(c, false, None));
        by_fp.insert(fp, idx);
    }

    stats.units = u32::try_from(units.len()).unwrap_or(u32::MAX);
    ConsolidateResult { units, stats }
}

#[cfg(test)]
#[path = "consolidate_tests.rs"]
mod consolidate_tests;
