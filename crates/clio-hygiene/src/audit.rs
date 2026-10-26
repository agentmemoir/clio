// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the read-only `hygiene_audit` ranked scan
//! Bank-scoped noise candidates ranked by descending
//! `hygiene_noise_score`, with masked previews, pagination, and zero writes.
//!
//! ## Scale and Ceilings
//!
//! Active item scanning default ceiling is 10,000 items ([`DEFAULT_MAX_SCAN`]).
//! For banks exceeding 10,000 items, the audit returns a partial page marked
//! `incomplete: true`. Near-duplicate pairing runs over an in-memory inverted
//! index keyed by sketch token: memory cost is one 64-token sketch per scanned
//! item plus the token postings, and pairing cost is bounded by the postings of
//! tokens shared with a dominating candidate instead of all-pairs comparison.
//!
//! The upgrade path for banks exceeding 10,000 items is to move the token index
//! into storage (SQLite `fts5` trigram tables or PostgreSQL `pg_trgm`/`tsvector`
//! indexes) so pairing no longer needs every sketch in memory; until then the
//! partial page plus `incomplete: true` keeps the call bounded.
//!
//! ## Staleness and Touch Proxies
//!
//! Staleness scoring uses the most recent touch timestamp (`updated_at` when
//! later than `created_at`, or `created_at` as the proxy). The future telemetry
//! upgrade path will use a `last_retrieved_at` access log column once retrieval
//! access logging is integrated into storage.
//!
//! ## Scheduled Cleaner Decision
//!
//! Hygiene has no automatic or scheduled path: hygiene cleanup still runs only
//! through the explicit, operator-confirmed tool (dry-run counting vs confirmed
//! mutation), so an operator who wants a fixed hygiene window keeps calling it
//! on their own schedule. Automatic structural maintenance (dirty-path refresh
//! and hub distillation) is owned by the runtime's maintenance trigger and does
//! not cover hygiene.
//!
//! ## Owns
//! - [`hygiene_audit`]: scanner over active items, inverted-index near-duplicate
//!   pairing, deterministic ranking (score desc, then `item_id`), pagination,
//!   and the scan-budget `incomplete` signal (partial page, never a mutation)
//! - [`AuditReport`] / [`NoiseCandidate`] shapes with secret-safe previews
//!
//! ## Does not own
//! - The staleness ramp arithmetic: `staleness_of` delegates to
//!   `clio_types::decay::ramp_factor`
//! - Signal detection or score math (`detect` / `scoring`)
//! - Any mutation: audit MUST NOT flag, archive, discard, or write logs
//! - Automatic or scheduled hygiene runs: hygiene has no scheduler, and the
//!   runtime's maintenance trigger covers structural maintenance only
//!
//! ## Boundary
//!
//! Changes to scanning, ranking, or pagination should normally require changes
//! here. Changes to detectors or action bands should not require changes here.
//!
//! Keep this module focused on the audit read path.

use crate::detect::{NoiseSignals, detect_noise, masked_preview};
use crate::scoring::{ScoreInput, score_noise};
use clio_config::ranking::HygieneWeights;
use clio_store::{InspectValidity, ItemInspectFilter, Store};
use clio_types::{AmError, ramp_factor};
use serde::Serialize;
use std::collections::{BTreeSet, HashMap, HashSet};

/// Default `min_score`: candidates below the appendix flag band are omitted.
pub const DEFAULT_MIN_SCORE: f64 = 0.40;

/// Default scan budget: active items scored per audit call (failure row:
/// huge banks return a partial page + `incomplete` instead of unbounded work).
///
/// Default ceiling is 10,000 items. Banks exceeding 10k items return a partial
/// page marked `incomplete: true`. The upgrade path for banks exceeding 10,000
/// items is database-side trigram/minhash indexing in storage.
pub const DEFAULT_MAX_SCAN: usize = 10000;

/// Scan page size (the store inspector's cap).
const SCAN_PAGE: usize = 200;

/// One ranked noise candidate (audit shape).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NoiseCandidate {
    /// Item id (cleanup always addresses items by id, never by content).
    pub item_id: String,
    /// `hygiene_noise_score` in `0.0..=1.0`.
    pub noise_score: f64,
    /// Explainable factor ids, ordered deterministically.
    pub noise_reasons: Vec<String>,
    /// Suggested action band, `None` below 0.40.
    pub suggested_action: Option<String>,
    /// Truncated / redacted preview (`None` for empty text).
    pub preview: Option<String>,
}

/// Result of one `hygiene_audit` call.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AuditReport {
    /// Bank scope scanned.
    pub bank: String,
    /// Active items scanned.
    pub scanned: usize,
    /// Total candidates at or above `min_score` (before pagination).
    pub total_candidates: usize,
    /// Ranked candidates for the requested page.
    pub candidates: Vec<NoiseCandidate>,
    /// True when the bank held more active items than the scan budget: the
    /// report covers only the first `max_scan` items (stable scan order) and
    /// candidates may be incomplete. Read-only either way — audit never mutates.
    pub incomplete: bool,
}

/// Inputs for [`hygiene_audit`].
pub struct AuditArgs<'a> {
    /// Discrete repository (identity, content, and metadata reads only).
    pub store: &'a dyn Store,
    /// Bank scope (tenant isolation).
    pub bank: &'a str,
    /// Effective "now" for staleness (ISO text; caller-supplied for determinism).
    pub now: &'a str,
    /// Rank floor: candidates below it are omitted (default 0.40).
    pub min_score: f64,
    /// Page size (1..=200).
    pub limit: usize,
    /// Page offset into the ranked candidate list.
    pub offset: usize,
    /// Noise weights (deployment-tunable knobs).
    pub weights: &'a HygieneWeights,
    /// Staleness half-life in days.
    pub stale_half_life_days: f64,
    /// Scan budget: at most this many active items are scored; when the bank
    /// holds more, the report sets `incomplete` (default [`DEFAULT_MAX_SCAN`]).
    pub max_scan: usize,
}

/// Validate audit inputs (bank scope, score floor, page bounds).
fn validate_audit_args(args: &AuditArgs<'_>) -> Result<(), AmError> {
    if args.bank.trim().is_empty() {
        return Err(AmError::new(
            clio_types::ErrorCode::MissingBank,
            "bank is required",
        ));
    }
    if !(0.0..=1.0).contains(&args.min_score) {
        return Err(AmError::new(
            clio_types::ErrorCode::OutOfRange,
            "min_score must be in [0, 1]",
        ));
    }
    if args.limit == 0 || args.limit > SCAN_PAGE {
        return Err(AmError::new(
            clio_types::ErrorCode::OutOfRange,
            format!(
                "hygiene_audit limit must be in 1..={SCAN_PAGE}, got {}",
                args.limit
            ),
        ));
    }
    Ok(())
}

/// Ranked read-only noise audit. Never mutates memory (not even flags).
///
/// Determinism: identical bank contents, `now`, and weights yield an identical
/// ranked id list (stable scan order + score-desc / id-asc sort).
///
/// # Errors
/// Empty bank, `min_score` outside `[0, 1]`, limit outside `1..=200`, or
/// store read failures.
pub fn hygiene_audit(args: &AuditArgs<'_>) -> Result<AuditReport, AmError> {
    validate_audit_args(args)?;

    // Scan every active, non-discarded item in the bank (metadata pages, then
    // per-item content reads for signal detection), stopping at the scan
    // budget: a larger bank yields a partial scan flagged `incomplete`.
    let mut rows = Vec::new();
    let mut offset_scan = 0;
    let mut incomplete = false;
    loop {
        let mut filter = ItemInspectFilter::new(args.bank);
        filter.validity = InspectValidity::Active;
        filter.limit = SCAN_PAGE;
        filter.offset = offset_scan;
        let page = args.store.inspect_items(&filter)?;
        let fetched = page.len();
        let remaining = args.max_scan.saturating_sub(rows.len());
        let take = fetched.min(remaining);
        rows.extend(page.into_iter().take(take));
        if take < fetched {
            incomplete = true;
            break;
        }
        if fetched < SCAN_PAGE {
            break;
        }
        offset_scan += fetched;
    }

    let mut texts: Vec<(String, String, f64, String)> = Vec::new();
    for row in &rows {
        let item = args
            .store
            .get_memory_item(args.bank, &row.item_id)?
            .ok_or_else(|| {
                AmError::new(
                    clio_types::ErrorCode::Internal,
                    format!("scanned item `{}` vanished mid-audit", row.item_id),
                )
            })?;
        let text = item_text(item.snapshot.as_ref(), item.gist.as_deref());
        let utility = item.admission_score.unwrap_or(0.5).clamp(0.0, 1.0);
        let touch = if !row.updated_at.is_empty() && row.updated_at > row.created_at {
            row.updated_at.clone()
        } else {
            row.created_at.clone()
        };
        texts.push((row.item_id.clone(), text, utility, touch));
    }

    let tokens: Vec<Vec<String>> = texts.iter().map(|(_, t, _, _)| token_sketch(t)).collect();
    let near_dup_flags = near_duplicate_flags(&texts, &tokens);

    let mut candidates: Vec<NoiseCandidate> = Vec::new();
    for (i, (item_id, text, utility, touch)) in texts.iter().enumerate() {
        let signals: NoiseSignals = detect_noise(text);
        let staleness = staleness_of(touch, args.now, args.stale_half_life_days);
        let scored = score_noise(
            &ScoreInput {
                secret_kinds: signals.secret_kinds.clone(),
                secret_run_len: signals.secret_run_len,
                token_run_len: signals.token_run_len,
                pattern_kinds: signals.pattern_kinds.clone(),
                near_duplicate: near_dup_flags[i],
                utility_proxy: *utility,
                staleness,
            },
            args.weights,
        );
        if scored.score < args.min_score {
            continue;
        }
        candidates.push(NoiseCandidate {
            item_id: item_id.clone(),
            noise_score: scored.score,
            noise_reasons: scored.reasons.iter().map(|r| (*r).to_owned()).collect(),
            suggested_action: scored.suggested_action.map(str::to_owned),
            preview: masked_preview(text, signals.secret_hit()),
        });
    }

    candidates.sort_by(|a, b| {
        b.noise_score
            .partial_cmp(&a.noise_score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.item_id.cmp(&b.item_id))
    });
    let total = candidates.len();
    let page: Vec<NoiseCandidate> = candidates
        .into_iter()
        .skip(args.offset)
        .take(args.limit)
        .collect();
    Ok(AuditReport {
        bank: args.bank.to_owned(),
        scanned: rows.len(),
        total_candidates: total,
        candidates: page,
        incomplete,
    })
}

/// Per-item near-duplicate flags: item `i` is flagged when a dominating item
/// `j` (see [`dominates`]) has Jaccard sketch similarity `>= 0.8`.
///
/// The inverted index maps each sketch token to the items carrying it, so only
/// candidates sharing at least `ceil(0.8 * |sketch_i|)` tokens get compared:
/// a Jaccard score of `0.8` implies at least that many shared tokens because
/// the intersection is bounded by `|sketch_i|`. Pairs are therefore never
/// missed while all-pairs sketch comparison is avoided.
fn near_duplicate_flags(
    texts: &[(String, String, f64, String)],
    tokens: &[Vec<String>],
) -> Vec<bool> {
    let unique_tokens: Vec<HashSet<&str>> = tokens
        .iter()
        .map(|toks| toks.iter().map(String::as_str).collect())
        .collect();
    let mut token_index: HashMap<&str, Vec<usize>> = HashMap::new();
    for (j, u_toks) in unique_tokens.iter().enumerate() {
        for &tok in u_toks {
            token_index.entry(tok).or_default().push(j);
        }
    }

    let mut shared_counts: HashMap<usize, usize> = HashMap::new();
    let mut flags = vec![false; texts.len()];
    for (i, u_toks_i) in unique_tokens.iter().enumerate() {
        if u_toks_i.is_empty() {
            continue;
        }
        let min_shared = u_toks_i.len().saturating_mul(4).div_ceil(5);
        shared_counts.clear();
        for &tok in u_toks_i {
            if let Some(postings) = token_index.get(tok) {
                for &j in postings {
                    if j != i && dominates(texts[j].2, &texts[j].0, texts[i].2, &texts[i].0) {
                        *shared_counts.entry(j).or_insert(0) += 1;
                    }
                }
            }
        }
        flags[i] = shared_counts
            .iter()
            .any(|(&j, &shared)| shared >= min_shared && similarity(&tokens[j], &tokens[i]) >= 0.8);
    }
    flags
}

/// True when candidate `j` dominates item `i`: higher utility, or equal utility
/// with a lexicographically earlier id (for deterministic tie-breaking).
fn dominates(utility_j: f64, id_j: &str, utility_i: f64, id_i: &str) -> bool {
    utility_j > utility_i || ((utility_j - utility_i).abs() < f64::EPSILON && id_j < id_i)
}

/// Detection text: gist prose plus serialized snapshot (both decrypted halves).
fn item_text(snapshot: Option<&serde_json::Value>, gist: Option<&str>) -> String {
    let mut text = String::new();
    if let Some(gist) = gist {
        text.push_str(gist);
    }
    if let Some(snapshot) = snapshot {
        if !text.is_empty() {
            text.push(' ');
        }
        text.push_str(&snapshot.to_string());
    }
    text
}

/// Lowercase word sketch (capped at 64 tokens) for near-duplicate comparison.
fn token_sketch(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_ascii_lowercase)
        .take(64)
        .collect()
}

/// Jaccard similarity of two token sketches.
fn similarity(a: &[String], b: &[String]) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let set_a: BTreeSet<&String> = a.iter().collect();
    let set_b: BTreeSet<&String> = b.iter().collect();
    let shared = set_a.intersection(&set_b).count();
    let union = set_a.len() + set_b.len() - shared;
    f64::from(u32::try_from(shared).unwrap_or(0)) / f64::from(u32::try_from(union).unwrap_or(1))
}

/// Staleness in `0.0..=1.0` from ISO touch timestamp vs `now` and the span.
///
/// Uses the most recent touch timestamp (`updated_at` when later than `created_at`,
/// or `created_at` as the proxy). Unparseable timestamps fail soft to 0 (never stale)
/// rather than failing the scan, and a non-positive or non-finite span also yields 0.
/// Future telemetry upgrade path: `last_retrieved_at`
/// access log column once storage logs read access.
fn staleness_of(touch_at: &str, now: &str, span_days: f64) -> f64 {
    let Some(age) = age_days(touch_at, now) else {
        return 0.0;
    };
    ramp_factor(age, span_days)
}

/// Whole days between two ISO `YYYY-MM-DD…` stamps (`None` when unparseable).
fn age_days(from: &str, to: &str) -> Option<f64> {
    let a = day_number(from)?;
    let b = day_number(to)?;
    Some(f64::from(b.saturating_sub(a)))
}

/// `YYYY-MM-DD` prefix → days since 1970-01-01 (civil calendar day count).
fn day_number(iso: &str) -> Option<i32> {
    const CUM: [i32; 12] = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
    let year: i32 = iso.get(0..4)?.parse().ok()?;
    let month: i32 = iso.get(5..7)?.parse().ok()?;
    let day: i32 = iso.get(8..10)?.parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let mut days = 0;
    let mut y = 1970;
    while y < year {
        days += i32::from(leap_year(y)) + 365;
        y += 1;
    }
    while y > year {
        y -= 1;
        days -= i32::from(leap_year(y)) + 365;
    }
    let month_index = usize::try_from(month - 1).unwrap_or(0);
    let doy = CUM[month_index] + i32::from(leap_year(year) && month > 2);
    Some(days + doy + day - 1)
}

fn leap_year(y: i32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

#[cfg(test)]
#[path = "audit_tests.rs"]
mod audit_tests;

#[cfg(test)]
#[path = "audit_edge_tests.rs"]
mod audit_edge_tests;

#[cfg(test)]
#[path = "audit_scale_tests.rs"]
pub(crate) mod audit_scale_tests;

#[cfg(test)]
#[path = "audit_scale_overlap_tests.rs"]
mod audit_scale_overlap_tests;
