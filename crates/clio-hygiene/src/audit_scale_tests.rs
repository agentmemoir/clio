// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for scale and boundary tests of the
//! `hygiene_audit` scan (`audit.rs`): audits exceeding the old 5k ceiling,
//! inverted-index near-duplicate pairing equivalence against brute force and at
//! the raised ceiling, and staleness calculations using the most recent touch
//! timestamp.
//!
//! ## Owns
//! - Tests for scan ceilings above 5,000 items and incomplete signal on overflow
//! - Scalable near-duplicate pairing equivalence tests against brute force
//! - Staleness tests verifying updated items use the most recent touch timestamp
//!
//! The high-overlap corpus test at the raised ceiling lives in
//! `scale_overlap_tests.rs`; it is shared with this module, not duplicated.
//!
//! ## Does not own
//! - Core audit ranking and pagination tests (`audit_tests.rs`)
//! - The high-overlap ceiling corpus and its cost bound (`scale_overlap_tests.rs`)
//! - Detector unit tests (`detect_tests.rs`)
//! - The audit engine itself (`audit.rs`)
//!
//! ## Boundary
//!
//! Tests here verify scaling properties and touch timestamps of the audit scan.
//!
//! Keep this module focused on scale and touch-timestamp test cases.

use crate::audit::{AuditArgs, DEFAULT_MAX_SCAN, hygiene_audit};
use crate::detect::detect_noise;
use crate::scoring::{ScoreInput, score_noise};
use clio_config::ranking::HygieneWeights;
use clio_store::{LocalDevKms, SqliteStore, Store};
use clio_types::{EpistemicKind, ItemKind, MemoryItem, SemanticCategory};
use std::collections::BTreeSet;
use std::sync::{Arc, LazyLock};
use std::time::Instant;

pub(super) static WEIGHTS: LazyLock<HygieneWeights> = LazyLock::new(HygieneWeights::default);

pub(super) fn store() -> Arc<SqliteStore> {
    SqliteStore::open_in_memory(Arc::new(LocalDevKms::new()))
        .map(Arc::new)
        .expect("open sqlite")
}

pub(super) fn make_item(
    id: &str,
    bank: &str,
    gist: &str,
    admission: f64,
    created_at: &str,
    updated_at: &str,
) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: format!("subj-{id}"),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::ToolConfig),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.8),
        admission_score: Some(admission),
        source_ref: Some("turn_1".into()),
        snapshot: None,
        gist: Some(gist.into()),
        valid_from: created_at.into(),
        valid_until: None,
        tx_from: created_at.into(),
        tx_until: None,
        created_at: created_at.into(),
        updated_at: updated_at.into(),
        context: None,
    }
}

pub(super) fn audit_args<'a>(
    store: &'a dyn Store,
    bank: &'a str,
    now: &'a str,
    min_score: f64,
    limit: usize,
    offset: usize,
) -> AuditArgs<'a> {
    AuditArgs {
        store,
        bank,
        now,
        min_score,
        limit,
        offset,
        weights: &WEIGHTS,
        stale_half_life_days: 90.0,
        max_scan: DEFAULT_MAX_SCAN,
    }
}

fn token_sketch_ref(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_ascii_lowercase)
        .take(64)
        .collect()
}

fn similarity_ref(a: &[String], b: &[String]) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let set_a: BTreeSet<&String> = a.iter().collect();
    let set_b: BTreeSet<&String> = b.iter().collect();
    let shared = set_a.intersection(&set_b).count();
    let union = set_a.len() + set_b.len() - shared;
    f64::from(u32::try_from(shared).unwrap_or(0)) / f64::from(u32::try_from(union).unwrap_or(1))
}

#[test]
fn t01_hygiene_audit_above_old_ceiling_and_overflow_incomplete() {
    let s = store();
    let bank = "bank-scale-ceiling";
    let s_ref = &*s as &dyn Store;

    // Verify raised ceiling constant
    assert_eq!(
        DEFAULT_MAX_SCAN, 10_000,
        "DEFAULT_MAX_SCAN raised from 5,000 to 10,000"
    );

    // Seed a bank above the old 5,000-item ceiling (5,001 items) with distinct
    // gists, so the audit must scan past where the old ceiling would have cut.
    let old_ceiling = 5_000;
    let item_count = old_ceiling + 1;
    for i in 0..item_count {
        let id = format!("scale-item-{i:05}");
        // Single unique token per item: keeps the near-duplicate index linear so
        // this test measures scan reach, not similarity cost (the near-duplicate
        // tests below cover near-duplicate correctness and cost).
        let gist = format!("scaleentry{i:06}");
        let itm = make_item(
            &id,
            bank,
            &gist,
            0.8,
            "2026-09-01T00:00:00Z",
            "2026-09-01T00:00:00Z",
        );
        s_ref.create_memory_item(&itm, "actor").unwrap();
    }

    // Default ceiling (10,000, above the old 5k ceiling): the bank completes
    // without `incomplete`.
    let full = hygiene_audit(&audit_args(s_ref, bank, "2026-09-20T00:00:00Z", 0.0, 50, 0)).unwrap();
    assert!(
        !full.incomplete,
        "audit of {item_count} items under the 10k ceiling completes without incomplete"
    );
    assert_eq!(full.scanned, item_count, "all items in bank scanned");

    // Setting max_scan below item_count returns partial page and sets incomplete: true.
    let mut constrained = audit_args(s_ref, bank, "2026-09-20T00:00:00Z", 0.0, 50, 0);
    constrained.max_scan = 100;
    let partial = hygiene_audit(&constrained).unwrap();
    assert!(
        partial.incomplete,
        "budget smaller than bank sets incomplete: true"
    );
    assert_eq!(partial.scanned, 100, "scanned capped at max_scan budget");
}

#[test]
fn t02_near_duplicate_lookup_at_scale_matches_brute_force() {
    let s = store();
    let bank = "bank-near-dup-scale";
    let s_ref = &*s as &dyn Store;

    // Create a corpus with unique items, identical duplicates, high-overlap items,
    // and distinct utilities to test tie-breaking.
    let base_templates = [
        "cluster alpha api gateway deployment configuration for environment",
        "database connection pool tuning parameters for production load",
        "redis cache eviction strategy and time to live settings",
        "background task queue worker concurrency and timeout limits",
        "monitoring agent metrics collection interval and scrape targets",
    ];

    let mut expected_items: Vec<(String, String, f64, String)> = Vec::new();

    for i in 0..100 {
        let template = base_templates[i % base_templates.len()];
        let (gist, utility) = if i % 5 == 0 {
            // High similarity variant
            (format!("{template} variant alpha"), 0.4)
        } else if i % 5 == 1 {
            // Near-duplicate with higher utility
            (format!("{template} variant alpha"), 0.9)
        } else if i % 5 == 2 {
            // Equal utility tie-breaker
            (format!("{template} variant beta"), 0.5)
        } else {
            // Unique text with low overlap
            (format!("{template} distinct run {i} extra parameters"), 0.6)
        };
        let id = format!("dup-scale-{i:03}");
        let date = "2026-09-01T00:00:00Z";
        let itm = make_item(&id, bank, &gist, utility, date, date);
        s_ref.create_memory_item(&itm, "actor").unwrap();
        expected_items.push((id, gist, utility, date.to_string()));
    }

    // Compute expected brute-force near-duplicate flags
    let tokens: Vec<Vec<String>> = expected_items
        .iter()
        .map(|(_, g, _, _)| token_sketch_ref(g))
        .collect();

    let brute_force_dups: Vec<bool> = expected_items
        .iter()
        .enumerate()
        .map(|(i, (item_id, _, utility, _))| {
            tokens.iter().enumerate().any(|(j, sketch)| {
                j != i
                    && similarity_ref(sketch, &tokens[i]) >= 0.8
                    && (expected_items[j].2 > *utility
                        || ((expected_items[j].2 - *utility).abs() < f64::EPSILON
                            && expected_items[j].0.as_str() < item_id.as_str()))
            })
        })
        .collect();

    // Measure time of hygiene_audit with now = date (staleness 0) to isolate near-duplicate effect
    let start = Instant::now();
    let report = hygiene_audit(&audit_args(
        s_ref,
        bank,
        "2026-09-01T00:00:00Z",
        0.0,
        200,
        0,
    ))
    .unwrap();
    let duration = start.elapsed();

    // Measured, not gated. This test's claim is that the audit's classifications
    // match brute force, which the assertions below check exhaustively. The
    // cost claim is carried by the candidate count, not by a duration: a fixed
    // wall-clock constant describes the host rather than the code, and fails on
    // a loaded runner while the code is unchanged (issue #53). Printed so a
    // slowdown is still visible in the log.
    println!(
        "scale audit over {} items took {duration:?}",
        report.scanned
    );

    // Verify candidate classifications match brute force exactly
    for (i, (item_id, gist, utility, _date)) in expected_items.iter().enumerate() {
        let expected_dup = brute_force_dups[i];
        let signals = detect_noise(gist);
        let expected_score = score_noise(
            &ScoreInput {
                secret_hit: signals.secret_hit(),
                pattern_kinds: signals.pattern_kinds.clone(),
                near_duplicate: expected_dup,
                utility_proxy: *utility,
                staleness: 0.0,
            },
            &WEIGHTS,
        );

        let candidate = report
            .candidates
            .iter()
            .find(|c| c.item_id == *item_id)
            .expect("candidate present in report");

        let has_dup_reason = candidate
            .noise_reasons
            .iter()
            .any(|r| r == "near_duplicate");
        assert_eq!(
            has_dup_reason, expected_dup,
            "item {item_id} duplicate status mismatch"
        );
        assert!((candidate.noise_score - expected_score.score).abs() < 1e-6);
    }
}

#[test]
fn t03_staleness_with_last_relevance() {
    let s = store();
    let bank = "bank-staleness-touch";
    let s_ref = &*s as &dyn Store;

    // Item 1: created long ago, never updated.
    let item_stale = make_item(
        "item-stale",
        bank,
        "backup procedures and maintenance schedule",
        0.5,
        "2026-01-01T00:00:00Z",
        "2026-01-01T00:00:00Z",
    );
    s_ref.create_memory_item(&item_stale, "actor").unwrap();

    // Item 2: created long ago, but recently updated (more recent touch timestamp).
    let item_touched = make_item(
        "item-touched",
        bank,
        "redis replication configuration and cluster nodes",
        0.5,
        "2026-01-01T00:00:00Z",
        "2026-09-18T00:00:00Z",
    );
    s_ref.create_memory_item(&item_touched, "actor").unwrap();

    // Item 3: created recently, updated_at is older/anomalous (falls back to created_at).
    let item_fallback = make_item(
        "item-fallback",
        bank,
        "new deployment runbook guide",
        0.5,
        "2026-09-18T00:00:00Z",
        "2026-01-01T00:00:00Z",
    );
    s_ref.create_memory_item(&item_fallback, "actor").unwrap();

    let report =
        hygiene_audit(&audit_args(s_ref, bank, "2026-09-20T00:00:00Z", 0.0, 50, 0)).unwrap();

    let cand_stale = report
        .candidates
        .iter()
        .find(|c| c.item_id == "item-stale")
        .expect("stale candidate");
    let cand_touched = report
        .candidates
        .iter()
        .find(|c| c.item_id == "item-touched")
        .expect("touched candidate");
    let cand_fallback = report
        .candidates
        .iter()
        .find(|c| c.item_id == "item-fallback")
        .expect("fallback candidate");

    // The untouched item is stale (created 262 days ago with 90-day half-life).
    assert!(
        cand_stale.noise_reasons.iter().any(|r| r == "stale"),
        "untouched item must be marked stale"
    );

    // The recently updated item is NOT stale because its updated_at (2 days ago) was used.
    assert!(
        !cand_touched.noise_reasons.iter().any(|r| r == "stale"),
        "recently touched item must not be marked stale"
    );

    // The updated item has a lower noise score than the stale item.
    assert!(
        cand_touched.noise_score < cand_stale.noise_score,
        "more recent touch lowers staleness and noise score: {} < {}",
        cand_touched.noise_score,
        cand_stale.noise_score
    );

    // Fallback item uses created_at when updated_at is older, so it is not stale.
    assert!(
        !cand_fallback.noise_reasons.iter().any(|r| r == "stale"),
        "fallback item uses recent created_at"
    );
}
