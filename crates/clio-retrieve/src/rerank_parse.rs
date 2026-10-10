// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for parsing provider rerank response bodies
//! into ordered index/score pairs and for the documented per-provider score
//! normalization behind the `Reranker` trait's return shape.
//!
//! ## Owns
//! - `RerankedDoc`, the ordered `(index, score)` pair the trait returns
//! - `parse_rerank_response`: the TEI bare array (best-first), the
//!   Cohere/Jina-style `results[]` shape, and the `scores[]` shape; scores
//!   kept when present (`None` otherwise); fail-closed index validation and
//!   `scores[]` element validation; full-permutation append of omitted docs
//! - `normalize_rerank_score` and the per-provider normalization spec
//!
//! ## Does not own
//! - The `Reranker` trait, HTTP adapters, or provider selection (`rerank`)
//! - The request wire contract (adapter-owned; TEI pins `raw_scores: false`)
//! - Fail-open policy or writing scores onto hits (`hybrid_rank`/`finalize`)
//!
//! ## Boundary
//!
//! Changes to response shapes or the normalization decision should normally
//! require changes here. Changes to transport or ordering policy should not
//! require changes here.
//!
//! # Score contract
//!
//! `Reranker::rerank` returns one `RerankedDoc` per input document,
//! best-first: `index` is the position in the request's `documents` slice and
//! `score` is the provider's relevance normalized to `[0, 1]`
//! (`normalize_rerank_score`). The response-shape compatibility rules are
//! unchanged from the index-only parser: a bare-array or `results[]` entry
//! that carries an index keeps that index and its place in the order even
//! when it carries no numeric score; such an entry's `score` is `None` and
//! the exposed `scores.reranker` stays `null` — a value is never fabricated.
//! A document the provider omitted (appended so the result is always a full
//! permutation) also carries `score: None`. Index validation stays
//! fail-closed: a missing/non-integer, duplicate, or out-of-range index
//! errors the whole rerank. No reranker configured (passthrough) or a rerank
//! error also leaves the field `null`.
//!
//! ## Per-provider normalization (deterministic)
//!
//! | Provider | Request | Input range | Transform | Output range |
//! |----------|---------|-------------|-----------|--------------|
//! | TEI sidecar | `/rerank`, `raw_scores: false` pinned | sigmoid score, approx. `(0, 1)` | identity on `[0, 1]`, clamped to `[0, 1]` | `[0, 1]` |
//! | Cohere | `/v1/rerank` | calibrated `relevance_score`, `[0, 1]` | identity on `[0, 1]`, clamped to `[0, 1]` | `[0, 1]` |
//!
//! TEI applies sigmoid when `raw_scores` is `false`, so pinning the field in
//! the request means the sidecar always returns normalized scores; raw logits
//! appear only when a client sends `raw_scores: true`, which Clio never
//! does. The clamp is a defensive guard for a non-conforming server that
//! returns raw logits (negative or above 1). The exposed value is display
//! metadata: it is not cross-provider comparable because TEI's per-pair
//! logistic and Cohere's calibrated relevance are calibrated differently,
//! and it never replaces the fused `final` score.

use clio_types::AmError;
use serde_json::Value;

/// One reranked candidate in best-first order: the document's position in the
/// request's `documents` slice plus its normalized provider relevance.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RerankedDoc {
    /// Position in the `documents` slice passed to `rerank`.
    pub index: usize,
    /// Normalized relevance in `[0, 1]` (see `normalize_rerank_score`); `None`
    /// when the provider supplied no numeric score for the entry, including a
    /// document it omitted (appended so the result is always a full
    /// permutation). Never a fabricated value.
    pub score: Option<f64>,
}

/// Normalize a provider relevance score into the exposed `reranker` range.
///
/// Both supported providers already emit scores in approximately `[0, 1]`:
/// the TEI sidecar applies sigmoid when `raw_scores` is `false` (pinned in
/// the request body), and Cohere's `relevance_score` is calibrated to
/// `[0, 1]`. The transform is therefore the identity on `[0, 1]`, with a
/// defensive clamp so a non-conforming server that returns raw logits
/// (negative or above 1) cannot leak an out-of-range value. Deterministic
/// for identical inputs. The result is display metadata: it is not
/// cross-provider comparable because the two providers calibrate
/// differently, and it MUST NOT replace the fused `final` score.
#[must_use]
pub fn normalize_rerank_score(raw: f64) -> f64 {
    raw.clamp(0.0, 1.0)
}

/// Parse a sidecar rerank body into a full permutation of document indices,
/// each carrying its normalized provider score when the provider supplied one.
///
/// A bare array `[{"index":i,"score":s}, ...]` (TEI `RerankResponse`) is read
/// best-first: array order is trusted and `index` plus `score` are consumed.
/// The object-key shapes `{"results":[{"index":i,"relevance_score":s}, ...]}`
/// (Cohere/Jina; `score` is accepted as an alias) and `{"scores":[s0, ...]}`
/// keep their existing ordering semantics and also yield scores. An entry
/// that carries an index but no numeric score keeps its index and place in
/// the order and carries `score: None` (never a fabricated value), so a body
/// that previously only reordered documents still reorders them today.
/// Out-of-range or duplicate indices fail closed; documents the sidecar
/// omitted are appended in original order with `score: None` so the result is
/// always a full permutation.
///
/// # Errors
/// Missing/malformed bare array, `results`/`scores`, out-of-range, or
/// duplicate indices.
#[allow(clippy::cast_possible_truncation)] // u64 index -> usize is lossless on 64-bit targets.
pub fn parse_rerank_response(value: &Value, doc_count: usize) -> Result<Vec<RerankedDoc>, AmError> {
    let mut ranked: Vec<RerankedDoc> = if let Some(array) = value.as_array() {
        // TEI bare array: trust the order (the sidecar returns it best-first).
        let mut out = Vec::with_capacity(array.len());
        for r in array {
            out.push(RerankedDoc {
                index: entry_index(r, "rerank array[].index must be an integer")?,
                score: entry_score(r, &["score"]).map(normalize_rerank_score),
            });
        }
        out
    } else if let Some(results) = value.get("results").and_then(Value::as_array) {
        let mut out = Vec::with_capacity(results.len());
        for r in results {
            out.push(RerankedDoc {
                index: entry_index(r, "rerank results[].index must be an integer")?,
                score: entry_score(r, &["relevance_score", "score"]).map(normalize_rerank_score),
            });
        }
        out
    } else if let Some(scores) = value.get("scores").and_then(Value::as_array) {
        let mut pairs: Vec<(usize, f64)> = Vec::with_capacity(scores.len());
        for (i, s) in scores.iter().enumerate() {
            let v = s
                .as_f64()
                .ok_or_else(|| bad("rerank scores[] must be numeric"))?;
            pairs.push((i, v));
        }
        // Stable sort keeps index ascending for equal scores; normalization
        // happens after sorting so the provider's own order is preserved.
        pairs.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        pairs
            .into_iter()
            .map(|(i, v)| RerankedDoc {
                index: i,
                score: Some(normalize_rerank_score(v)),
            })
            .collect()
    } else {
        return Err(bad(
            "rerank response must be an array, results[], or scores[]",
        ));
    };
    let mut seen = vec![false; doc_count];
    for doc in &ranked {
        if doc.index >= doc_count {
            return Err(bad("rerank index out of range"));
        }
        if seen[doc.index] {
            return Err(bad("rerank returned a duplicate index"));
        }
        seen[doc.index] = true;
    }
    for (i, hit) in seen.iter().enumerate() {
        if !hit {
            // Omitted document: appended in original order with no score.
            ranked.push(RerankedDoc {
                index: i,
                score: None,
            });
        }
    }
    Ok(ranked)
}

/// Read the `index` field of one response entry, failing closed otherwise.
#[allow(clippy::cast_possible_truncation)] // u64 index -> usize is lossless on 64-bit targets.
fn entry_index(entry: &Value, message: &str) -> Result<usize, AmError> {
    entry
        .get("index")
        .and_then(Value::as_u64)
        .map(|raw| raw as usize)
        .ok_or_else(|| bad(message))
}

/// Read the first numeric score found under `keys` of one response entry.
///
/// `None` when no key carries a number: the caller records `score: None`, so
/// an entry that carries only an index keeps its index and its place in the
/// order instead of failing the rerank, and no value is ever invented.
fn entry_score(entry: &Value, keys: &[&str]) -> Option<f64> {
    keys.iter()
        .find_map(|key| entry.get(*key).and_then(Value::as_f64))
}

fn bad(message: &str) -> AmError {
    AmError::new(clio_types::ErrorCode::Internal, message)
}
