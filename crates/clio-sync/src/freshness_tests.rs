// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! Tests for the pure freshness module. Every case supplies its own `now`, so
//! nothing here reads a clock or depends on the host.

use super::{
    DEFAULT_FRESHNESS_MAX_AGE_SECS, DEFAULT_FRESHNESS_MAX_FUTURE_SECS, Freshness, FreshnessPolicy,
    parse_utc_secs,
};

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
    let age = DEFAULT_FRESHNESS_MAX_AGE_SECS;
    assert_eq!(policy.classify(now - age, now), Freshness::Fresh);
    assert_eq!(policy.classify(now - age - 1, now), Freshness::Stale);
}

#[test]
fn classify_is_fresh_at_the_future_boundary() {
    let policy = FreshnessPolicy::default();
    let now = 1_800_000_000;
    let ahead = DEFAULT_FRESHNESS_MAX_FUTURE_SECS;
    assert_eq!(policy.classify(now + ahead, now), Freshness::Fresh);
    assert_eq!(policy.classify(now + ahead + 1, now), Freshness::Future);
}

#[test]
fn classify_is_fresh_for_the_same_second() {
    let policy = FreshnessPolicy::default();
    let now = 1_800_000_000;
    assert_eq!(policy.classify(now, now), Freshness::Fresh);
}

#[test]
fn zero_age_bound_disables_only_the_past_side() {
    let policy = FreshnessPolicy {
        max_age_secs: 0,
        max_future_secs: 60,
    };
    let now = 1_800_000_000;
    assert_eq!(policy.classify(i64::MIN, now), Freshness::Fresh, "very old");
    assert_eq!(policy.classify(now + 60, now), Freshness::Fresh);
    assert_eq!(policy.classify(now + 61, now), Freshness::Future);
}

#[test]
fn zero_future_bound_disables_only_the_future_side() {
    let policy = FreshnessPolicy {
        max_age_secs: 900,
        max_future_secs: 0,
    };
    let now = 1_800_000_000;
    assert_eq!(
        policy.classify(i64::MAX, now),
        Freshness::Fresh,
        "far ahead"
    );
    assert_eq!(policy.classify(now - 900, now), Freshness::Fresh);
    assert_eq!(policy.classify(now - 901, now), Freshness::Stale);
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
fn disabled_policy_accepts_old_and_far_future_stamps() {
    let policy = FreshnessPolicy::disabled();
    let now: i64 = 1_800_000_000;
    let century: i64 = 100 * 365 * 86_400;
    let ancient = now - century;
    let distant = now + century;
    assert_eq!(policy.classify(ancient, now), Freshness::Fresh);
    assert_eq!(policy.classify(distant, now), Freshness::Fresh);
}

#[test]
fn default_policy_uses_the_two_product_bounds() {
    let policy = FreshnessPolicy::default();
    assert_eq!(policy.max_age_secs, 900);
    assert_eq!(policy.max_future_secs, 60);
    assert_eq!(DEFAULT_FRESHNESS_MAX_AGE_SECS, 900);
    assert_eq!(DEFAULT_FRESHNESS_MAX_FUTURE_SECS, 60);
}

#[test]
fn default_policy_rejects_just_outside_each_bound() {
    let policy = FreshnessPolicy::default();
    let now = 1_800_000_000;
    assert_eq!(policy.classify(now - 901, now), Freshness::Stale);
    assert_eq!(policy.classify(now - 899, now), Freshness::Fresh);
    assert_eq!(policy.classify(now + 61, now), Freshness::Future);
    assert_eq!(policy.classify(now + 59, now), Freshness::Fresh);
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
    let age = DEFAULT_FRESHNESS_MAX_AGE_SECS;
    let ahead = DEFAULT_FRESHNESS_MAX_FUTURE_SECS;
    // The known stamp is the epoch second KNOWN_SECS, so now is derived from it.
    assert_eq!(
        policy.classify_text(KNOWN_STAMP, KNOWN_SECS),
        Freshness::Fresh
    );
    assert_eq!(
        policy.classify_text(KNOWN_STAMP, KNOWN_SECS + age),
        Freshness::Fresh,
        "exactly the old boundary"
    );
    assert_eq!(
        policy.classify_text(KNOWN_STAMP, KNOWN_SECS + age + 1),
        Freshness::Stale
    );
    assert_eq!(
        policy.classify_text(KNOWN_STAMP, KNOWN_SECS - ahead),
        Freshness::Fresh,
        "exactly the future boundary"
    );
    assert_eq!(
        policy.classify_text(KNOWN_STAMP, KNOWN_SECS - ahead - 1),
        Freshness::Future
    );
}

#[test]
fn classify_text_keeps_garbage_fresh_when_one_side_is_off() {
    let policy = FreshnessPolicy {
        max_age_secs: 900,
        max_future_secs: 0,
    };
    let now = 1_800_000_000;
    assert_eq!(policy.classify_text("garbage", now), Freshness::Fresh);
    assert_eq!(
        policy.classify_text(KNOWN_STAMP, KNOWN_SECS + 901),
        Freshness::Stale
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
fn extreme_policy_bounds_do_not_panic() {
    let wide = FreshnessPolicy {
        max_age_secs: i64::MAX,
        max_future_secs: i64::MAX,
    };
    let now = 0;
    assert_eq!(wide.classify(0, now), Freshness::Fresh);
    assert_eq!(wide.classify(i64::MAX, now), Freshness::Fresh);

    let negative = FreshnessPolicy {
        max_age_secs: -1,
        max_future_secs: -1,
    };
    assert_eq!(negative.classify(i64::MIN, i64::MAX), Freshness::Fresh);
    assert_eq!(negative.classify(i64::MAX, i64::MIN), Freshness::Fresh);
}
