// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for candidate-filter tests: domain membership,
//! bi-temporal visibility on both axes, and recency/age scoring.
//!
//! ## Owns
//! - Pure filter-function assertions (`item_in_domains`, `item_visible_at`,
//!   `recency_score`, `age_days`)
//!
//! ## Does not own
//! - Orchestrator-level filter wiring (`hybrid_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::filter::{age_days, item_in_domains, item_visible_at, recency_score, system_now_iso};
use crate::fixtures_tests as fx;
use clio_types::{SearchDomain, TimeAxis};

fn domain(raw: &str) -> SearchDomain {
    SearchDomain::parse(raw).expect("domain")
}

#[test]
fn domain_membership() {
    let episodic = fx::item("e", "b", "gist");
    let semantic = fx::semantic_item("s", "b", "gist");
    let persona = {
        let mut p = fx::semantic_item("p", "b", "gist");
        p.category = Some(clio_types::SemanticCategory::Persona);
        p
    };

    assert!(item_in_domains(&episodic, &[]));
    assert!(item_in_domains(&episodic, &[domain("episodic")]));
    assert!(item_in_domains(&episodic, &[domain("task")]));
    assert!(!item_in_domains(&episodic, &[domain("semantic")]));
    assert!(!item_in_domains(&episodic, &[domain("failure")]));
    assert!(item_in_domains(&semantic, &[domain("semantic")]));
    assert!(item_in_domains(&persona, &[domain("persona")]));
    // `semantic` is the broad domain: it covers every semantic category,
    // including `persona`.
    assert!(item_in_domains(&persona, &[domain("semantic")]));
    assert!(item_in_domains(
        &semantic,
        &[domain("task"), domain("semantic")]
    ));
}

#[test]
fn temporal_visibility_valid_and_transaction_axes() {
    let mut entry = fx::item("e", "b", "gist");
    entry.valid_from = "2026-01-01".into();
    entry.valid_until = Some("2026-03-01".into());
    entry.tx_from = "2026-02-01".into();

    assert!(item_visible_at(&entry, TimeAxis::Valid, "2026-02-01"));
    assert!(!item_visible_at(&entry, TimeAxis::Valid, "2026-04-01"));
    assert!(!item_visible_at(&entry, TimeAxis::Valid, "2025-12-31"));
    assert!(item_visible_at(&entry, TimeAxis::Transaction, "2026-02-01"));
    assert!(!item_visible_at(
        &entry,
        TimeAxis::Transaction,
        "2026-01-01"
    ));
}

#[test]
fn recency_handles_bad_and_future_inputs() {
    let mut entry = fx::item("e", "b", "gist");
    entry.updated_at = "2026-01-01T00:00:00Z".into();

    // Non-positive tau disables the boost: the decay guard returns 0.0.
    assert_eq!(recency_score(&entry, "2026-02-01T00:00:00Z", 0.0), 0.0);
    // Unparseable timestamps disable the boost: `age_days` yields `None`.
    assert_eq!(recency_score(&entry, "not-a-date", 30.0), 0.0);
    // A clock before the update: exp(-ln2 * -365 / 30) clamps to 1.0.
    assert_eq!(recency_score(&entry, "2025-01-01T00:00:00Z", 30.0), 1.0);
    // 30 days at tau 30 is one half-life: exp(-ln2 * 30 / 30) = 0.5.
    let mid = recency_score(&entry, "2026-01-31T00:00:00Z", 30.0);
    assert!((mid - 0.5).abs() < 1e-12, "got {mid}");
    // Empty updated_at falls back to created_at (2026-01-01): the same
    // 30 days at tau 30, so the fallback value is also 0.5.
    entry.updated_at = String::new();
    let via_created = recency_score(&entry, "2026-01-31T00:00:00Z", 30.0);
    assert!((via_created - 0.5).abs() < 1e-12, "got {via_created}");
}

#[test]
fn recency_curve_pins_published_tau_30_row() {
    let mut entry = fx::item("e", "b", "gist");
    entry.updated_at = "2026-01-01T00:00:00Z".into();

    // The published retrieval row at tau 30, pinned through the real call
    // path at 7/30/90/365 days. Values are quoted to six decimals, so each
    // assertion carries the rounding bound those decimals imply. The 30-day
    // half-life is also pinned at the same input in
    // `recency_handles_bad_and_future_inputs`; the complete row lives here.
    let row = [
        (7, "2026-01-08T00:00:00Z", 0.850_667),
        (30, "2026-01-31T00:00:00Z", 0.500_000),
        (90, "2026-04-01T00:00:00Z", 0.125_000),
        (365, "2027-01-01T00:00:00Z", 0.000_218),
    ];
    for (days, now, expected) in row {
        let score = recency_score(&entry, now, 30.0);
        assert!(
            (score - expected).abs() < 1e-6,
            "tau 30 at {days} days: expected {expected}, got {score}"
        );
    }
}

#[test]
fn age_days_parses_supported_formats() {
    assert!(age_days("2026-01-01", "2026-01-11").is_some());
    assert!(age_days("2026-01-01T00:00:00Z", "2026-01-02T00:00:00Z").is_some());
    assert!(age_days("2026-01-01T00:00:00", "2026-01-02T00:00:00").is_some());
    assert!(age_days("nonsense", "2026-01-01").is_none());
    assert!(age_days("2026-01-01", "nonsense").is_none());
    let days = age_days("2026-01-01", "2026-01-11").expect("days");
    assert!((days - 10.0).abs() < 1e-9);
}

#[test]
fn system_now_iso_is_parseable() {
    let now = system_now_iso();
    assert!(!now.is_empty(), "{now:?}");
    assert!(age_days(&now, &now).is_some());
}

/// `system_now_iso` returns UTC on the exact `%Y-%m-%dT%H:%M:%SZ` shape and
/// tracks the real clock. Only the shape and the closeness are asserted, never
/// a host-specific value.
#[test]
fn system_now_iso_has_the_second_precision_utc_shape() {
    let now = system_now_iso();

    let parsed = chrono::DateTime::parse_from_rfc3339(&now).expect("rfc3339 stamp");
    assert_eq!(parsed.format("%Y-%m-%dT%H:%M:%SZ").to_string(), now);
    assert_eq!(parsed.offset().local_minus_utc(), 0, "stamp must be UTC");

    // Second precision: no fractional seconds and a fixed 20-character length.
    assert_eq!(now.len(), 20, "{now:?}");
    assert!(!now.contains('.'), "{now:?}");

    // Close to the host clock. The bound is generous and only says the stamp
    // tracks now; no host-specific value is pinned.
    let host_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after epoch")
        .as_secs() as i64;
    let delta = (parsed.timestamp() - host_secs).abs();
    assert!(delta < 60, "stamp {now:?} is {delta}s from the host clock");
}

#[test]
fn domain_membership_needs_a_matching_category_for_persona() {
    // The `persona` domain filters on the category column. A semantic item
    // whose category is `schema` is not a persona.
    let schema = fx::semantic_item("s", "b", "gist");
    assert!(!item_in_domains(&schema, &[domain("persona")]));

    // The category column is `None`: the SQL predicate it mirrors requires a
    // value there, so the item cannot match.
    let uncategorised = {
        let mut u = fx::semantic_item("u", "b", "gist");
        u.category = None;
        u
    };
    assert!(!item_in_domains(&uncategorised, &[domain("persona")]));
    // The broad domain places no filter on the category, so it still matches.
    assert!(item_in_domains(&uncategorised, &[domain("semantic")]));
}

#[test]
fn domain_membership_needs_a_matching_episodic_type() {
    let mut untagged = fx::item("e", "b", "gist");
    untagged.episodic_type = None;

    // `task` filters on the episodic-type column, which is `None` here.
    assert!(!item_in_domains(&untagged, &[domain("task")]));
    assert!(!item_in_domains(&untagged, &[domain("failure")]));
    // `episodic` places no filter on that column, so the kind alone decides.
    assert!(item_in_domains(&untagged, &[domain("episodic")]));

    let mut failure = fx::item("f", "b", "gist");
    failure.episodic_type = Some(clio_types::EpisodicType::Failure);
    assert!(item_in_domains(&failure, &[domain("failure")]));
    assert!(!item_in_domains(&failure, &[domain("temporal")]));
}

#[test]
fn valid_axis_intervals_are_closed_at_the_start_and_open_at_the_end() {
    let mut entry = fx::item("e", "b", "gist");
    entry.valid_from = "2026-01-01".into();

    // No `valid_until`: the interval is open, so any later moment is visible.
    assert!(item_visible_at(&entry, TimeAxis::Valid, "2030-01-01"));

    entry.valid_until = Some("2026-03-01".into());
    assert!(item_visible_at(&entry, TimeAxis::Valid, "2026-01-01"));
    assert!(item_visible_at(&entry, TimeAxis::Valid, "2026-02-28"));
    // Half-open interval: the end instant itself is already outside.
    assert!(!item_visible_at(&entry, TimeAxis::Valid, "2026-03-01"));
    assert!(!item_visible_at(&entry, TimeAxis::Valid, "2026-03-02"));
}

#[test]
fn transaction_axis_uses_the_same_open_interval_boundaries() {
    let mut entry = fx::item("e", "b", "gist");
    entry.tx_from = "2026-01-01".into();
    entry.tx_until = Some("2026-03-01".into());

    assert!(item_visible_at(&entry, TimeAxis::Transaction, "2026-01-01"));
    assert!(!item_visible_at(
        &entry,
        TimeAxis::Transaction,
        "2026-03-01"
    ));
    assert!(!item_visible_at(
        &entry,
        TimeAxis::Transaction,
        "2025-12-31"
    ));

    // Opening the transaction interval makes later moments visible again.
    entry.tx_until = None;
    assert!(item_visible_at(&entry, TimeAxis::Transaction, "2026-03-01"));
}

#[test]
fn recency_rejects_a_non_finite_or_negative_tau() {
    let mut entry = fx::item("e", "b", "gist");
    entry.updated_at = "2026-01-01T00:00:00Z".into();

    // The decay guard treats a non-finite or negative half-life as no decay
    // information at all, so the boost is disabled rather than inverted.
    for tau in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -30.0] {
        assert_eq!(
            recency_score(&entry, "2026-01-31T00:00:00Z", tau),
            0.0,
            "tau {tau}"
        );
    }
}

#[test]
fn recency_rejects_an_unparseable_updated_at() {
    let mut entry = fx::item("e", "b", "gist");
    entry.updated_at = "not-a-date".into();

    // A non-empty but unparseable stamp is used as-is, and then fails to parse.
    assert_eq!(recency_score(&entry, "2026-01-31T00:00:00Z", 30.0), 0.0);
}

#[test]
fn age_days_counts_fractional_days() {
    let half = age_days("2026-01-01T12:00:00Z", "2026-01-02T00:00:00Z").expect("days");
    assert!((half - 0.5).abs() < 1e-9, "got {half}");

    // A date-only stamp is midnight UTC, so this is 36 hours.
    let day_and_a_half = age_days("2026-01-01", "2026-01-02T12:00:00Z").expect("days");
    assert!((day_and_a_half - 1.5).abs() < 1e-9, "got {day_and_a_half}");
}

#[test]
fn age_days_is_negative_when_now_precedes_the_stamp() {
    let age = age_days("2026-01-02T00:00:00Z", "2026-01-01T00:00:00Z").expect("days");
    assert!((age + 1.0).abs() < 1e-9, "got {age}");

    // The same stamp on both sides is exactly zero, not a rounding error.
    assert_eq!(
        age_days("2026-01-01T00:00:00Z", "2026-01-01T00:00:00Z"),
        Some(0.0)
    );
}

#[test]
fn age_days_rejects_every_unsupported_format() {
    // A US-style date, a bare date-time without seconds, and an epoch number
    // are all outside the three accepted shapes.
    assert!(age_days("01/01/2026", "2026-01-02").is_none());
    assert!(age_days("2026-01-01T00:00", "2026-01-02").is_none());
    assert!(age_days("1767225600", "2026-01-02").is_none());
    // A date with an impossible day is rejected rather than rolled over.
    assert!(age_days("2026-02-30", "2026-03-01").is_none());
    assert!(age_days("", "2026-03-01").is_none());
    assert!(age_days("2026-03-01", "").is_none());
}
