// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for bounded-parallel chunk extract+verify fan-out.
//!
//! ## Owns
//! - Hard concurrency cap `P` over chunk workers
//! - Per-chunk result aggregation (verified / refused / transport error)
//!
//! ## Does not own
//! - Consolidation or gated admit
//!
//! ## Boundary
//!
//! Changes to parallel extract scheduling should normally require changes here.
//! Changes to consolidation or gated admit should not require changes here.
//!
//! Keep this module focused on bounded-parallel chunk extract fan-out.

use crate::chunk::Chunk;
use crate::extract::{ExtractCandidate, Extractor};
use crate::pipeline::{
    ExtractVerifyArgs, ExtractVerifyOutcome, NonAuthoritativeRetention, extract_and_verify,
};
use crate::schema::FieldSpec;
use crate::verify::VerifyConfig;
use clio_types::AmError;
use serde_json::Value;
use std::sync::{Mutex, mpsc};

/// One chunk's extract+verify outcome.
#[derive(Debug, Clone, PartialEq)]
pub struct ChunkWorkerResult {
    /// Chunk id.
    pub chunk_id: String,
    /// Source text used as span haystack.
    pub source_text: String,
    /// Verified candidate when ok.
    pub candidate: Option<ExtractCandidate>,
    /// Non-authoritative retention when verify refused.
    pub retained: Option<NonAuthoritativeRetention>,
    /// Transport / extract error message when the worker failed hard.
    pub error: Option<String>,
    /// Extract call count (0 when transport failed before extract).
    pub extract_calls: u32,
}

/// Aggregate of a parallel extract batch.
#[derive(Debug, Clone, PartialEq)]
pub struct ParallelExtractAggregate {
    /// Per-chunk results in input order.
    pub results: Vec<ChunkWorkerResult>,
    /// Count of chunks that produced a verified candidate.
    pub verified_count: usize,
    /// Count of chunks that refused after verify.
    pub refused_count: usize,
    /// Count of chunks that hit transport/extract errors.
    pub error_count: usize,
}

impl ParallelExtractAggregate {
    /// Verified candidates with their source text and chunk id.
    #[must_use]
    pub fn verified(&self) -> Vec<(String, String, ExtractCandidate)> {
        self.results
            .iter()
            .filter_map(|r| {
                r.candidate
                    .clone()
                    .map(|c| (r.chunk_id.clone(), r.source_text.clone(), c))
            })
            .collect()
    }
}

fn worker_result(
    chunk: &Chunk,
    outcome: Result<ExtractVerifyOutcome, AmError>,
) -> ChunkWorkerResult {
    match outcome {
        Ok(o) => ChunkWorkerResult {
            chunk_id: chunk.chunk_id.clone(),
            source_text: chunk.source_text.clone(),
            candidate: o.candidate,
            retained: o.retained,
            error: None,
            extract_calls: o.extract_calls,
        },
        Err(e) => ChunkWorkerResult {
            chunk_id: chunk.chunk_id.clone(),
            source_text: chunk.source_text.clone(),
            candidate: None,
            retained: None,
            error: Some(e.message),
            extract_calls: 0,
        },
    }
}

/// Extract+verify each chunk with at most `concurrency` workers.
///
/// `context` (when supplied) rides every chunk's `ExtractRequest` as untrusted
/// descriptive metadata; the span haystack stays the chunk's own source text.
/// Partial-success policy: failed chunks are recorded; they do not cancel siblings.
/// Callers that need all-or-nothing must inspect the aggregate and abort before admit.
/// Per-chunk verify telemetry is omitted here (workers are multi-threaded); ingest
/// records aggregate merge/admit counts instead.
pub fn extract_chunks_parallel(
    extractor: &(dyn Extractor + Sync),
    chunks: &[Chunk],
    schema_template: &Value,
    fields: &[FieldSpec],
    verify: &VerifyConfig,
    concurrency: usize,
    context: Option<&str>,
) -> ParallelExtractAggregate {
    if chunks.is_empty() {
        return ParallelExtractAggregate {
            results: vec![],
            verified_count: 0,
            refused_count: 0,
            error_count: 0,
        };
    }
    // Fail closed before any worker starts: an empty or oversized context is
    // rejected up front, so every chunk reports the error and no extractor is
    // called. `extract_and_verify` keeps its own validation as
    // the inner gate for the sequential path.
    if let Some(context) = context
        && let Err(err) = clio_types::validate_context(context)
    {
        let results: Vec<ChunkWorkerResult> = chunks
            .iter()
            .map(|chunk| worker_result(chunk, Err(err.clone())))
            .collect();
        return ParallelExtractAggregate {
            error_count: results.len(),
            results,
            verified_count: 0,
            refused_count: 0,
        };
    }
    let limit = concurrency.max(1).min(chunks.len());
    let work: Mutex<Vec<(usize, Chunk)>> = Mutex::new(
        chunks
            .iter()
            .cloned()
            .enumerate()
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect(),
    );
    let (tx, rx) = mpsc::channel::<(usize, ChunkWorkerResult)>();
    std::thread::scope(|scope| {
        for _ in 0..limit {
            let tx = tx.clone();
            let work = &work;
            scope.spawn(move || {
                loop {
                    let next = work.lock().expect("work queue").pop();
                    let Some((idx, chunk)) = next else {
                        break;
                    };
                    let outcome = extract_and_verify(ExtractVerifyArgs {
                        extractor,
                        source_text: &chunk.source_text,
                        schema_template: schema_template.clone(),
                        tool_payload: chunk.tool_payload.clone(),
                        context: context.map(str::to_owned),
                        fields,
                        verify,
                        emit: None,
                    });
                    let _ = tx.send((idx, worker_result(&chunk, outcome)));
                }
            });
        }
    });
    drop(tx);
    let mut pairs: Vec<(usize, ChunkWorkerResult)> = rx.into_iter().collect();
    pairs.sort_by_key(|(i, _)| *i);
    let results: Vec<ChunkWorkerResult> = pairs.into_iter().map(|(_, r)| r).collect();
    let verified_count = results.iter().filter(|r| r.candidate.is_some()).count();
    let refused_count = results
        .iter()
        .filter(|r| r.candidate.is_none() && r.error.is_none())
        .count();
    let error_count = results.iter().filter(|r| r.error.is_some()).count();
    ParallelExtractAggregate {
        results,
        verified_count,
        refused_count,
        error_count,
    }
}

/// Run `f` over `items` with at most `limit` concurrent threads (order-preserving).
pub fn map_bounded<T, R, F>(items: Vec<T>, limit: usize, f: F) -> Vec<R>
where
    T: Send,
    R: Send,
    F: Fn(T) -> R + Sync,
{
    if items.is_empty() {
        return Vec::new();
    }
    let n = items.len();
    let limit = limit.max(1).min(n);
    let work: Mutex<Vec<(usize, T)>> = Mutex::new(items.into_iter().enumerate().rev().collect());
    let (tx, rx) = mpsc::channel::<(usize, R)>();
    std::thread::scope(|scope| {
        for _ in 0..limit {
            let tx = tx.clone();
            let work = &work;
            let f = &f;
            scope.spawn(move || {
                loop {
                    let next = work.lock().expect("work").pop();
                    let Some((idx, item)) = next else {
                        break;
                    };
                    let _ = tx.send((idx, f(item)));
                }
            });
        }
    });
    drop(tx);
    let mut pairs: Vec<(usize, R)> = rx.into_iter().collect();
    pairs.sort_by_key(|(i, _)| *i);
    pairs.into_iter().map(|(_, r)| r).collect()
}

#[cfg(test)]
#[path = "parallel_tests.rs"]
mod parallel_tests;
