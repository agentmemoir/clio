// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for candidate-level filters shared by hybrid
//! retrieval: domain membership, bi-temporal visibility, and recency scoring.
//!
//! ## Owns
//! - `item_in_domains` / `item_visible_at` predicate checks
//! - `recency_score`'s timestamp choice (`updated_at` falling back to
//!   `created_at`) and the ISO timestamp parsing it needs
//!
//! ## Does not own
//! - The decay arithmetic: `recency_score` delegates to
//!   `clio_types::decay::half_life_factor`
//! - Fusion, rerank, or compose packing
//! - Persistence or interval algebra (`clio-types::interval_contains`)
//!
//! ## Boundary
//!
//! Changes to candidate filtering or the recency timestamp choice should
//! normally require changes here. Changes to the decay arithmetic itself
//! belong to `clio-types::decay`, and ranking fusion changes should not
//! require changes here.

use clio_types::{MemoryItem, SearchDomain, TimeAxis, half_life_factor, interval_contains};

/// Whether `item` matches any requested domain (`[]` = no filter).
#[must_use]
pub fn item_in_domains(item: &MemoryItem, domains: &[SearchDomain]) -> bool {
    if domains.is_empty() {
        return true;
    }
    domains.iter().any(|d| {
        let (kind, category, episodic) = d.item_predicates();
        item.kind.as_str() == kind
            && category.is_none_or(|c| item.category.is_some_and(|ic| ic.as_str() == c))
            && episodic.is_none_or(|e| item.episodic_type.is_some_and(|ie| ie.as_str() == e))
    })
}

/// Whether `item` is visible on `axis` at `as_of` (half-open interval).
#[must_use]
pub fn item_visible_at(item: &MemoryItem, axis: TimeAxis, as_of: &str) -> bool {
    match axis {
        TimeAxis::Valid => interval_contains(&item.valid_from, item.valid_until.as_deref(), as_of),
        TimeAxis::Transaction => interval_contains(&item.tx_from, item.tx_until.as_deref(), as_of),
    }
}

/// Exponential recency of `item` relative to `now`, in `[0, 1]`.
///
/// Uses `updated_at` (falling back to `created_at`). Unparseable timestamps
/// or an invalid `tau_days` yield `0.0`, so a recency read never fails a query.
#[must_use]
pub fn recency_score(item: &MemoryItem, now: &str, tau_days: f64) -> f64 {
    let ts = if item.updated_at.is_empty() {
        &item.created_at
    } else {
        &item.updated_at
    };
    let Some(age) = age_days(ts, now) else {
        return 0.0;
    };
    half_life_factor(age, tau_days)
}

/// Fractional days between two timestamps (`now - ts`); `None` if unparseable.
#[must_use]
pub fn age_days(ts: &str, now: &str) -> Option<f64> {
    let a = parse_ts(ts)?;
    let b = parse_ts(now)?;
    Some((b - a).num_seconds() as f64 / 86_400.0)
}

/// Current UTC time as `%Y-%m-%dT%H:%M:%SZ` (empty string only if the clock is before epoch).
#[must_use]
pub fn system_now_iso() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    chrono::DateTime::from_timestamp(secs as i64, 0)
        .map_or_else(String::new, |d| d.format("%Y-%m-%dT%H:%M:%SZ").to_string())
}

fn parse_ts(s: &str) -> Option<chrono::NaiveDateTime> {
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return Some(dt.naive_utc());
    }
    if let Ok(d) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return d.and_hms_opt(0, 0, 0);
    }
    chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S").ok()
}

#[cfg(test)]
#[path = "filter_tests.rs"]
mod filter_tests;

#[cfg(test)]
#[path = "filter_audit_asof_tests.rs"]
mod filter_audit_asof_tests;
