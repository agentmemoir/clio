// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests `hygiene_audit` edge cases (`audit.rs`): near-duplicate
//! pairing, staleness proxies, fail-soft timestamp parsing, and half-life
//! disabling.
//!
//! ## Owns
//! - Test coverage for near-duplicate pairing, staleness math, and fail-soft
//!   timestamp handling
//!
//! ## Does not own
//! - Ranking, pagination, or masking behavior (`audit_tests.rs`)
//! - The audit behavior itself (`audit.rs`)
//!
//! ## Boundary
//!
//! Tests here break when audit edge handling changes; they never change it.
//!
//! Keep this module focused on `hygiene_audit` edge tests.

use crate::audit::{AuditArgs, hygiene_audit, staleness_of};
use clio_config::ranking::HygieneWeights;
use clio_store::{LocalDevKms, SqliteStore, Store};
use clio_types::{EpistemicKind, ItemKind, MemoryItem, SemanticCategory};
use std::sync::{Arc, LazyLock};

static WEIGHTS: LazyLock<HygieneWeights> = LazyLock::new(HygieneWeights::default);

fn store() -> Arc<SqliteStore> {
    SqliteStore::open_in_memory(Arc::new(LocalDevKms::new()))
        .map(Arc::new)
        .expect("open sqlite")
}

fn item(id: &str, bank: &str, gist: &str, admission: f64, created_at: &str) -> MemoryItem {
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
        source_ref: Some("turn_9".into()),
        snapshot: None,
        gist: Some(gist.into()),
        valid_from: created_at.into(),
        valid_until: None,
        tx_from: created_at.into(),
        tx_until: None,
        created_at: created_at.into(),
        updated_at: created_at.into(),
        context: None,
    }
}

fn audit_args<'a>(
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
        max_scan: 1000,
    }
}

#[test]
fn near_duplicate_marks_lower_utility_copy() {
    let s = store();
    let bank = "bank-dup";
    let s_ref = &*s as &dyn Store;
    s_ref
        .create_memory_item(
            &item(
                "dup-hi",
                bank,
                "postgres runs on 5432 in staging",
                0.9,
                "2026-09-20T00:00:00Z",
            ),
            "actor",
        )
        .unwrap();
    s_ref
        .create_memory_item(
            &item(
                "dup-lo",
                bank,
                "postgres runs on 5432 in staging",
                0.0,
                "2026-09-20T00:00:00Z",
            ),
            "actor",
        )
        .unwrap();
    let report =
        hygiene_audit(&audit_args(s_ref, bank, "2026-09-20T00:00:00Z", 0.0, 50, 0)).unwrap();
    let lo = report
        .candidates
        .iter()
        .find(|c| c.item_id == "dup-lo")
        .expect("low-utility copy");
    assert!(lo.noise_reasons.iter().any(|r| r == "near_duplicate"));
    let hi = report
        .candidates
        .iter()
        .find(|c| c.item_id == "dup-hi")
        .expect("retained copy");
    assert!(!hi.noise_reasons.iter().any(|r| r == "near_duplicate"));
}
#[test]
fn empty_text_and_malformed_timestamps_fail_soft() {
    let s = store();
    let bank = "bank-soft";
    let s_ref = &*s as &dyn Store;
    // Empty gist + no snapshot: empty sketch guards and no preview.
    s_ref
        .create_memory_item(
            &item("soft-empty", bank, "", 0.5, "2026-09-20T00:00:00Z"),
            "actor",
        )
        .unwrap();
    // Malformed timestamps parse as not-stale (fail soft to staleness 0).
    s_ref
        .create_memory_item(
            &item("soft-bad", bank, "bad timestamp note", 0.5, "not-a-date"),
            "actor",
        )
        .unwrap();
    let report =
        hygiene_audit(&audit_args(s_ref, bank, "2026-09-20T00:00:00Z", 0.0, 50, 0)).unwrap();
    let empty = report
        .candidates
        .iter()
        .find(|c| c.item_id == "soft-empty")
        .expect("empty candidate");
    assert_eq!(empty.preview, None, "empty text yields no preview");
    assert!(!empty.noise_reasons.iter().any(|r| r == "stale"));
    let bad = report
        .candidates
        .iter()
        .find(|c| c.item_id == "soft-bad")
        .expect("bad-ts candidate");
    assert!(
        !bad.noise_reasons.iter().any(|r| r == "stale"),
        "unparseable timestamp => staleness 0"
    );
}
#[test]
fn near_duplicate_tie_breaks_on_earlier_id() {
    let s = store();
    let bank = "bank-tie";
    let s_ref = &*s as &dyn Store;
    // Identical utility and text: the lexicographically later id is the copy.
    s_ref
        .create_memory_item(
            &item(
                "tie-a",
                bank,
                "identical duplicate payload token",
                0.5,
                "2026-09-20T00:00:00Z",
            ),
            "actor",
        )
        .unwrap();
    s_ref
        .create_memory_item(
            &item(
                "tie-b",
                bank,
                "identical duplicate payload token",
                0.5,
                "2026-09-20T00:00:00Z",
            ),
            "actor",
        )
        .unwrap();
    let report =
        hygiene_audit(&audit_args(s_ref, bank, "2026-09-20T00:00:00Z", 0.0, 50, 0)).unwrap();
    let a = report
        .candidates
        .iter()
        .find(|c| c.item_id == "tie-a")
        .unwrap();
    let b = report
        .candidates
        .iter()
        .find(|c| c.item_id == "tie-b")
        .unwrap();
    assert!(b.noise_reasons.iter().any(|r| r == "near_duplicate"));
    assert!(!a.noise_reasons.iter().any(|r| r == "near_duplicate"));
}
#[test]
fn zero_stale_half_life_disables_staleness() {
    let s = store();
    let bank = "bank-half";
    let s_ref = &*s as &dyn Store;
    s_ref
        .create_memory_item(
            &item(
                "old-1",
                bank,
                "old note about caching",
                0.5,
                "2026-01-01T00:00:00Z",
            ),
            "actor",
        )
        .unwrap();
    let mut args = audit_args(s_ref, bank, "2026-09-20T00:00:00Z", 0.0, 50, 0);
    args.stale_half_life_days = 0.0;
    let report = hygiene_audit(&args).unwrap();
    let c = report
        .candidates
        .iter()
        .find(|c| c.item_id == "old-1")
        .unwrap();
    assert!(
        !c.noise_reasons.iter().any(|r| r == "stale"),
        "disabled half-life never marks stale"
    );
}
#[test]
fn staleness_uses_created_at_half_life() {
    let s = store();
    let bank = "bank-stale";
    let s_ref = &*s as &dyn Store;
    // created one year before `now` → beyond the 90-day half-life → staleness 1.0
    s_ref
        .create_memory_item(
            &item(
                "stale-1",
                bank,
                "old note about caching",
                0.5,
                "2026-01-01T00:00:00Z",
            ),
            "actor",
        )
        .unwrap();
    let report =
        hygiene_audit(&audit_args(s_ref, bank, "2026-09-20T00:00:00Z", 0.0, 50, 0)).unwrap();
    let c = report
        .candidates
        .iter()
        .find(|c| c.item_id == "stale-1")
        .expect("candidate");
    assert!(c.noise_reasons.iter().any(|r| r == "stale"));
    assert!(c.noise_score.is_finite());
}
#[test]
fn staleness_of_fails_soft_and_halves_at_mid_span() {
    // An unparseable touch stamp is never stale and never a failure: the factor
    // is exactly zero rather than a panic or an error.
    let unparseable = staleness_of("not-a-date", "2026-09-20T00:00:00Z", 90.0);
    assert_eq!(unparseable, 0.0, "unparseable touch stamp is never stale");
    assert!(unparseable.is_finite(), "fail-soft value stays finite");
    // 45 days into a 90-day span is exactly half the linear ramp.
    assert_eq!(
        staleness_of("2026-08-06T00:00:00Z", "2026-09-20T00:00:00Z", 90.0),
        0.5,
        "45 days of a 90-day span"
    );
}
#[test]
fn staleness_of_matches_the_published_hygiene_row() {
    // The hygiene staleness row at the shipped 90-day span: ages of 7, 30, 90
    // and 365 days ramp to 0.077778, 0.333333, 1.000000 and 1.000000 (clamped
    // once the age reaches the span). The values are pinned as literals.
    let now = "2026-09-20T00:00:00Z";
    let row = [
        ("2026-09-13T00:00:00Z", 0.077_778),
        ("2026-08-21T00:00:00Z", 0.333_333),
        ("2026-06-22T00:00:00Z", 1.0),
        ("2025-09-20T00:00:00Z", 1.0),
    ];
    for (touch, published) in row {
        let factor = staleness_of(touch, now, 90.0);
        assert!(
            (factor - published).abs() < 1e-6,
            "staleness_of({touch}, {now}, 90.0) = {factor}, published row says {published}"
        );
    }
}

#[test]
fn staleness_of_rejects_dates_outside_the_calendar() {
    // Month and day are range-checked before any arithmetic: month 0/13 and
    // day 0/32 parse as unparseable, never as a wrapped date.
    let now = "2026-09-20T00:00:00Z";
    for stamp in [
        "2026-13-01T00:00:00Z",
        "2026-00-10T00:00:00Z",
        "2026-05-32T00:00:00Z",
        "2026-05-00T00:00:00Z",
    ] {
        assert_eq!(
            staleness_of(stamp, now, 90.0),
            0.0,
            "{stamp} is outside the calendar and must fail soft"
        );
    }
}

#[test]
fn staleness_of_rejects_truncated_timestamps() {
    // A stamp that is too short to carry year, month, or day slices is
    // unparseable, not zero-padded.
    let now = "2026-09-20T00:00:00Z";
    for stamp in ["", "2026", "2026-01", "abcd-01-01", "202X-01-01"] {
        assert_eq!(
            staleness_of(stamp, now, 90.0),
            0.0,
            "{stamp} cannot be sliced into YYYY-MM-DD and must fail soft"
        );
    }
}

#[test]
fn staleness_of_counts_backwards_before_1970() {
    // A touch stamp before the epoch walks the year loop downwards; the age
    // lands far past the 90-day span and the ramp clamps at 1.0.
    assert_eq!(
        staleness_of("1960-06-15T00:00:00Z", "2026-09-20T00:00:00Z", 90.0),
        1.0,
        "pre-epoch touches are maximally stale"
    );
}

#[test]
fn staleness_of_counts_leap_day_february() {
    // Across a leap February the bridge is two days (Feb 28 -> Mar 1 spans
    // Feb 29); without the leap adjustment the same bridge would count one.
    let leap = staleness_of("2024-02-28T00:00:00Z", "2024-03-01T00:00:00Z", 90.0);
    assert!(
        (leap - 2.0 / 90.0).abs() < 1e-9,
        "leap-year February bridge is two days, got {leap}"
    );
    let plain = staleness_of("2023-02-28T00:00:00Z", "2023-03-01T00:00:00Z", 90.0);
    assert!(
        (plain - 1.0 / 90.0).abs() < 1e-9,
        "common-year February bridge is one day, got {plain}"
    );
}

#[test]
fn snapshot_halves_feed_the_detection_text() {
    let s = store();
    let bank = "bank-snap";
    let s_ref = &*s as &dyn Store;
    // Snapshot-only item: no gist, the serialized snapshot is the whole text.
    let mut secret = item("snap-secret", bank, "", 0.5, "2026-09-20T00:00:00Z");
    secret.gist = None;
    secret.snapshot = Some(serde_json::json!({"note": "api_key: sk-live-abcdefgh1234"}));
    s_ref.create_memory_item(&secret, "actor").unwrap();
    // Gist plus snapshot: both halves land in the scanned text.
    let mut both = item(
        "snap-both",
        bank,
        "plain prose about caching",
        0.5,
        "2026-09-20T00:00:00Z",
    );
    both.snapshot = Some(serde_json::json!({"k": "v"}));
    s_ref.create_memory_item(&both, "actor").unwrap();
    let report =
        hygiene_audit(&audit_args(s_ref, bank, "2026-09-20T00:00:00Z", 0.0, 50, 0)).unwrap();
    let secret_c = report
        .candidates
        .iter()
        .find(|c| c.item_id == "snap-secret")
        .expect("snapshot-only item is scanned");
    assert!(
        secret_c
            .noise_reasons
            .iter()
            .any(|r| r == "secret_credential"),
        "the secret inside the snapshot half is detected"
    );
    assert!(
        report.candidates.iter().any(|c| c.item_id == "snap-both"),
        "a gist-plus-snapshot item is scanned"
    );
}
