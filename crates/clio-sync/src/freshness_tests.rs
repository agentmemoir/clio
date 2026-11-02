// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! Tests for the pure freshness module. Every case supplies its own `now`, so
//! nothing here reads a clock or depends on the host.

use super::{DEFAULT_FRESHNESS_WINDOW_SECS, Freshness, FreshnessPolicy, parse_utc_secs};

/// One known stamp and its epoch second, from an independent calendar source.
const KNOWN_STAMP: &str = "2026-01-01T00:00:00Z";
const KNOWN_SECS: i64 = 1_767_225_600;

#[test]
fn parse_round_trips_a_known_stamp() {
    assert_eq!(parse_utc_secs(KNOWN_STAMP), Some(KNOWN_SECS));
    assert_eq!(parse_utc_secs("1970-01-01T00:00:00Z"), Some(0));
    assert_eq!(
        parse_utc_secs("2000-02-29T23:59:59Z"),
        Some(951_868_799),
        "the century leap day, one second before midnight"
    );
}

#[test]
fn parse_reads_the_leap_day() {
    assert_eq!(parse_utc_secs("2024-02-29T00:00:00Z"), Some(1_709_164_800));
    assert_eq!(
        parse_utc_secs("2024-02-29T12:34:56Z"),
        Some(1_709_210_096),
        "an arbitrary time on the leap day"
    );
}

#[test]
fn parse_rejects_a_missing_zulu_marker() {
    assert_eq!(parse_utc_secs("2026-01-01T00:00:00"), None);
    assert_eq!(parse_utc_secs("2026-01-01T00:00:00+00:00"), None);
}

#[test]
fn parse_rejects_a_space_separator() {
    assert_eq!(parse_utc_secs("2026-01-01 00:00:00Z"), None);
}

#[test]
fn parse_rejects_fractional_seconds() {
    assert_eq!(parse_utc_secs("2026-01-01T00:00:00.5Z"), None);
    assert_eq!(parse_utc_secs("2026-01-01T00:00:00.000Z"), None);
}

#[test]
fn parse_rejects_a_short_string() {
    assert_eq!(parse_utc_secs("2026-01-01T00:00:0Z"), None);
    assert_eq!(parse_utc_secs("2026-01-01"), None);
    assert_eq!(parse_utc_secs("Z"), None);
}

#[test]
fn parse_rejects_an_empty_string() {
    assert_eq!(parse_utc_secs(""), None);
}

#[test]
fn parse_rejects_non_digits_and_out_of_range_fields() {
    assert_eq!(parse_utc_secs("2026-13-01T00:00:00Z"), None, "month 13");
    assert_eq!(
        parse_utc_secs("2023-02-29T00:00:00Z"),
        None,
        "common-year leap day"
    );
    assert_eq!(parse_utc_secs("2026-01-00T00:00:00Z"), None, "day 0");
    assert_eq!(parse_utc_secs("2026-01-01T24:00:00Z"), None, "hour 24");
    assert_eq!(parse_utc_secs("2026-01-01T00:60:00Z"), None, "minute 60");
    assert_eq!(parse_utc_secs("2026-01-01T00:00:60Z"), None, "second 60");
    assert_eq!(parse_utc_secs("20xx-01-01T00:00:00Z"), None, "not digits");
}

#[test]
fn classify_is_fresh_at_the_old_boundary() {
    let policy = FreshnessPolicy::default();
    let now = 1_800_000_000;
    let window = DEFAULT_FRESHNESS_WINDOW_SECS;
    assert_eq!(policy.classify(now - window, now), Freshness::Fresh);
    assert_eq!(policy.classify(now - window - 1, now), Freshness::Stale);
}

#[test]
fn classify_is_fresh_at_the_new_boundary() {
    let policy = FreshnessPolicy::default();
    let now = 1_800_000_000;
    let window = DEFAULT_FRESHNESS_WINDOW_SECS;
    assert_eq!(policy.classify(now + window, now), Freshness::Fresh);
    assert_eq!(policy.classify(now + window + 1, now), Freshness::Future);
}

#[test]
fn classify_is_fresh_for_the_same_second() {
    let policy = FreshnessPolicy::default();
    let now = 1_800_000_000;
    assert_eq!(policy.classify(now, now), Freshness::Fresh);
}

#[test]
fn disabled_policy_is_always_fresh() {
    let policy = FreshnessPolicy::disabled();
    let now = 1_800_000_000;
    assert_eq!(policy.classify(0, now), Freshness::Fresh);
    assert_eq!(policy.classify(i64::MAX, now), Freshness::Fresh);
    assert_eq!(policy.classify(i64::MIN, now), Freshness::Fresh);
    assert_eq!(
        policy.classify_text("not a stamp at all", now),
        Freshness::Fresh
    );
}

#[test]
fn default_policy_uses_the_seven_day_window() {
    assert_eq!(FreshnessPolicy::default().window_secs, 604_800);
    assert_eq!(DEFAULT_FRESHNESS_WINDOW_SECS, 604_800);
}

#[test]
fn classify_text_is_fresh_for_garbage() {
    let policy = FreshnessPolicy::default();
    let now = 1_800_000_000;
    assert_eq!(policy.classify_text("", now), Freshness::Fresh);
    assert_eq!(policy.classify_text("garbage", now), Freshness::Fresh);
    assert_eq!(policy.classify_text("2026-01-01", now), Freshness::Fresh);
}

#[test]
fn classify_text_reads_a_stamp_at_each_boundary() {
    let policy = FreshnessPolicy::default();
    let window = DEFAULT_FRESHNESS_WINDOW_SECS;
    // The known stamp is the epoch second KNOWN_SECS, so now is derived from it.
    assert_eq!(
        policy.classify_text(KNOWN_STAMP, KNOWN_SECS),
        Freshness::Fresh
    );
    assert_eq!(
        policy.classify_text(KNOWN_STAMP, KNOWN_SECS + window),
        Freshness::Fresh,
        "exactly the old boundary"
    );
    assert_eq!(
        policy.classify_text(KNOWN_STAMP, KNOWN_SECS + window + 1),
        Freshness::Stale
    );
    assert_eq!(
        policy.classify_text(KNOWN_STAMP, KNOWN_SECS - window - 1),
        Freshness::Future
    );
}

#[test]
fn extreme_values_do_not_panic() {
    let policy = FreshnessPolicy::default();
    assert_eq!(policy.classify(i64::MAX, i64::MIN), Freshness::Future);
    assert_eq!(policy.classify(i64::MIN, i64::MAX), Freshness::Stale);
    assert_eq!(policy.classify(0, i64::MAX), Freshness::Stale);
    assert_eq!(policy.classify(0, i64::MIN), Freshness::Future);
    assert_eq!(policy.classify(i64::MAX, i64::MAX), Freshness::Fresh);
    assert_eq!(policy.classify(i64::MIN, i64::MIN), Freshness::Fresh);
}

#[test]
fn extreme_policy_windows_do_not_panic() {
    let wide = FreshnessPolicy {
        window_secs: i64::MAX,
    };
    let now = 0;
    assert_eq!(wide.classify(0, now), Freshness::Fresh);
    assert_eq!(wide.classify(i64::MAX, now), Freshness::Fresh);

    let negative = FreshnessPolicy { window_secs: -1 };
    assert_eq!(negative.classify(i64::MIN, i64::MAX), Freshness::Fresh);
}
