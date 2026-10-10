// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the lexical "same or similar task" match used
//! retry surfacing.
//!
//! ## Owns
//! - Definition tokenization and Jaccard similarity
//! - Bounded ranking of similar tasks against a query
//! - The retitle regression fixture set
//!
//! ## Does not own
//! - Failure retrieval or tool orchestration (see `failure` / `retry`)
//! - Admission scoring (`clio-admission`) or persistence (`clio-store`)
//!
//! ## Boundary
//!
//! Changes to the similarity metric or default threshold handling should
//! normally require changes here. Changes to failure storage should not.
//!
//! # Default threshold
//!
//! `HistoryPolicy::similarity_threshold` defaults to `0.5` (Jaccard over
//! lowercased alphanumeric tokens). Retitled tasks that keep the same
//! definition score `1.0`, so the fixtures pass without tuning; deployments may
//! lower the threshold for more recall.

use clio_types::{HistoryPolicy, TaskRecord};
use std::cmp::Ordering;
use std::collections::BTreeSet;

/// Lowercased alphanumeric tokens of `text` (punctuation and case dropped).
#[must_use]
pub fn normalize_tokens(text: &str) -> BTreeSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// Jaccard similarity of two texts, in `0.0..=1.0`.
///
/// Two empty texts are identical (`1.0`); one empty text scores `0.0`.
#[must_use]
pub fn lexical_similarity(a: &str, b: &str) -> f64 {
    let ta = normalize_tokens(a);
    let tb = normalize_tokens(b);
    if ta.is_empty() || tb.is_empty() {
        return if ta.is_empty() && tb.is_empty() {
            1.0
        } else {
            0.0
        };
    }
    let intersection = ta.intersection(&tb).count() as f64;
    let union = ta.union(&tb).count() as f64;
    intersection / union
}

/// Tasks whose definition meets the policy threshold, best match first.
///
/// Ties break on the newer `created_at`, then `task_id`, for determinism.
#[must_use]
pub fn rank_similar_tasks(
    query: &str,
    tasks: &[TaskRecord],
    policy: &HistoryPolicy,
) -> Vec<(f64, TaskRecord)> {
    let mut scored: Vec<(f64, TaskRecord)> = tasks
        .iter()
        .map(|t| (lexical_similarity(query, &t.definition), t.clone()))
        .filter(|(score, _)| *score >= policy.similarity_threshold)
        .collect();
    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(Ordering::Equal)
            .then_with(|| b.1.created_at.cmp(&a.1.created_at))
            .then_with(|| a.1.task_id.cmp(&b.1.task_id))
    });
    scored
}

/// One coding-agent retitle pair: same definition, different `task_id`.
pub struct RetitleFixture {
    /// Task id used on the first attempt.
    pub prior_task_id: &'static str,
    /// Task id used on the retry.
    pub retitled_task_id: &'static str,
    /// Shared task definition (identical across both ids).
    pub definition: &'static str,
}

/// Regression fixtures for similar-task surfacing (≥5 retitle pairs).
///
/// Each pair names the same coding task with two different `task_id` values, so
/// a retry under the new id must still surface the prior attempt's lesson.
pub const RETITLE_FIXTURES: &[RetitleFixture] = &[
    RetitleFixture {
        prior_task_id: "fix-flaky-login-test",
        retitled_task_id: "stabilize-login-test",
        definition: "fix the flaky login integration test",
    },
    RetitleFixture {
        prior_task_id: "add-upload-retry",
        retitled_task_id: "upload-retry-backoff",
        definition: "add retry with backoff to the upload client",
    },
    RetitleFixture {
        prior_task_id: "migrate-config-loader",
        retitled_task_id: "config-loader-migration",
        definition: "migrate the config loader to the new schema",
    },
    RetitleFixture {
        prior_task_id: "optimize-search-query",
        retitled_task_id: "search-query-perf",
        definition: "optimize the slow search query to avoid a full table scan",
    },
    RetitleFixture {
        prior_task_id: "harden-auth-cookies",
        retitled_task_id: "auth-cookie-security",
        definition: "harden session cookies with secure and httponly flags",
    },
    RetitleFixture {
        prior_task_id: "docker-build-cache",
        retitled_task_id: "speed-up-docker-build",
        definition: "speed up the docker build using layer caching",
    },
];

#[cfg(test)]
#[path = "similar_tests.rs"]
mod similar_tests;
