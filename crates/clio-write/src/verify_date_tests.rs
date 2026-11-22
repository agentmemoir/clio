// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for contract tests of date/time span matching.
//!
//! ## Owns
//! - Format preference, ambiguous numeric fail-closed, and precision compare
//! - Char-boundary-safe month-name windows and multilingual month aliases
//! - Character-offset spans returned by `match_date`
//!
//! ## Does not own
//! - Entity/number matchers or verify orchestration
//!
//! ## Boundary
//!
//! Changes to date format policy should normally update tests here.

use super::*;
use crate::verify::VerifyConfig;

#[test]
fn month_aliases_normalize_to_parsable_english() {
    const CASES: &[(&str, &str, &str)] = &[
        ("März", "Mar", "2026-03-03"),
        ("août", "Aug", "2026-08-03"),
        ("Août", "Aug", "2026-08-03"),
        ("dezember", "Dec", "2026-12-03"),
        ("Dezember", "Dec", "2026-12-03"),
        ("décembre", "Dec", "2026-12-03"),
        ("Décembre", "Dec", "2026-12-03"),
        ("septiembre", "Sep", "2026-09-03"),
        ("Septiembre", "Sep", "2026-09-03"),
    ];
    for (alias, english, expected) in CASES {
        let normalized = localize_months(&format!("{alias} 3, 2026"));
        assert_eq!(normalized, format!("{english} 3, 2026"), "alias {alias}");
        let parsed = Date::strptime("%b %d, %Y", &normalized)
            .unwrap_or_else(|err| panic!("alias {alias} failed: {err}"));
        assert_eq!(parsed.to_string(), *expected, "alias {alias}");
    }
}

#[test]
fn ambiguous_numeric_date_fails_closed() {
    let value = serde_json::json!("2026-03-04");
    let err = match_date("on 03/04/2026", &value, &VerifyConfig::default())
        .expect_err("ambiguous numeric date must fail closed");
    assert_eq!(err, "ambiguous date without day_first policy");
}

#[test]
fn iso_datetime_in_source_matches_snapshot() {
    let value = serde_json::json!("2026-03-04T05:06:07");
    let source = "at 2026-03-04T05:06:07 UTC";
    let (start, end) = match_date(source, &value, &VerifyConfig::default())
        .expect("ISO date-time span must match");
    assert_eq!(&source[start..end], "2026-03-04T05:06:07");
}

#[test]
fn snapshot_date_matches_source_datetime_on_same_day() {
    let value = serde_json::json!("2026-03-04");
    let source = "on 2026-03-04T05:06:07";
    let (start, end) = match_date(source, &value, &VerifyConfig::default())
        .expect("date-only snapshot must match a date-time span on that day");
    assert_eq!(&source[start..end], "2026-03-04T05:06:07");
}

#[test]
fn snapshot_datetime_does_not_match_date_only_span() {
    let value = serde_json::json!("2026-03-04T05:06:07");
    let err = match_date("on 2026-03-04", &value, &VerifyConfig::default())
        .expect_err("a date-only span must not satisfy a snapshot with a time");
    assert_eq!(err, "no locatable date span parses to snapshot value");
}

#[test]
fn month_name_in_source_matches_snapshot() {
    let value = serde_json::json!("2026-03-03");
    let source = "on March 3, 2026";
    let (start, end) =
        match_date(source, &value, &VerifyConfig::default()).expect("month-name span must match");
    assert_eq!(&source[start..end], "March 3, 2026");
}

#[test]
fn snapshot_accepts_space_separated_datetime() {
    let parsed = snapshot_instant(&serde_json::json!("2026-03-04 05:06:07"))
        .expect("space-separated ISO date-time must parse");
    let expected = DateTime::strptime("%Y-%m-%dT%H:%M:%S", "2026-03-04T05:06:07")
        .expect("the equivalent ISO date-time must parse");
    assert_eq!(parsed, ParsedInstant::DateTime(expected));
}

#[test]
fn snapshot_rejects_empty_and_non_iso_values() {
    assert_eq!(
        snapshot_instant(&serde_json::json!("")),
        Err("empty date value".to_owned())
    );
    assert_eq!(
        snapshot_instant(&serde_json::json!("yesterday")),
        Err("snapshot date must be ISO date or date-time".to_owned())
    );
}

#[test]
fn numeric_date_respects_day_first_policy() {
    let value = serde_json::json!("2026-03-04");
    let source = "on 03/04/2026";
    let month_first = VerifyConfig {
        day_first: Some(false),
        ..Default::default()
    };
    let (start, end) =
        match_date(source, &value, &month_first).expect("month-first policy must match 03/04/2026");
    assert_eq!(&source[start..end], "03/04/2026");
    let day_first = VerifyConfig {
        day_first: Some(true),
        ..Default::default()
    };
    assert_eq!(
        match_date(source, &value, &day_first).err(),
        Some("no locatable date span parses to snapshot value".to_owned())
    );
}

#[test]
fn unambiguous_numeric_dates_match_without_policy() {
    let value = serde_json::json!("2026-03-25");
    for (source, span) in [
        ("on 25/03/2026", "25/03/2026"),
        ("on 03/25/2026", "03/25/2026"),
    ] {
        let (start, end) = match_date(source, &value, &VerifyConfig::default())
            .expect("an unambiguous numeric date must match without day_first");
        assert_eq!(&source[start..end], span);
    }
}

#[test]
fn ambiguous_span_that_cannot_match_does_not_locate_a_span() {
    let value = serde_json::json!("2027-01-01");
    let err = match_date("on 03/04/2026", &value, &VerifyConfig::default())
        .expect_err("an ambiguous span that cannot match must not locate a span");
    assert_eq!(err, "no locatable date span parses to snapshot value");
}

#[test]
fn out_of_range_numeric_span_does_not_locate_a_match() {
    let value = serde_json::json!("2026-03-04");
    let err = match_date("on 99/99/2026", &value, &VerifyConfig::default())
        .expect_err("out-of-range numeric parts must not locate a span");
    assert_eq!(err, "no locatable date span parses to snapshot value");
}

#[test]
fn spanish_september_month_name_in_source_matches_snapshot() {
    for (date, source) in [
        ("2026-09-03", "on septiembre 3, 2026"),
        ("2026-09-03", "on Septiembre 3, 2026"),
        ("2026-09-13", "on septiembre 13, 2026"),
    ] {
        let value = serde_json::json!(date);
        let span = &source["on ".len()..];
        let (start, end) = match_date(source, &value, &VerifyConfig::default())
            .expect("a Spanish month-name span must match");
        assert_eq!(&source[start..end], span);
    }
}

#[test]
fn spanish_september_span_does_not_match_another_month() {
    let value = serde_json::json!("2026-10-03");
    let err = match_date("on septiembre 3, 2026", &value, &VerifyConfig::default())
        .expect_err("a September span must not satisfy an October snapshot");
    assert_eq!(err, "no locatable date span parses to snapshot value");
}

#[test]
fn long_alphabetic_run_does_not_locate_a_span() {
    let source = "a".repeat(400);
    let value = serde_json::json!("2026-09-03");
    assert_eq!(
        match_date(&source, &value, &VerifyConfig::default()).err(),
        Some("no locatable date span parses to snapshot value".to_owned())
    );
}

#[test]
fn ymd_option_fails_closed_on_out_of_range_parts() {
    assert!(ymd_option(40000, 3, 4).is_none());
    assert!(ymd_option(2026, 200, 4).is_none());
    assert!(ymd_option(2026, 3, 200).is_none());
    assert_eq!(
        ymd_option(2026, 3, 4),
        Some(Date::new(2026, 3, 4).expect("valid date"))
    );
}

#[test]
fn split_numeric_date_handles_dot_and_rejects_invalid_shapes() {
    assert_eq!(split_numeric_date("03.04.2026"), Some((3, 4, 2026)));
    assert!(split_numeric_date("2026-03-04").is_none());
    assert!(split_numeric_date("03/04").is_none());
    assert!(split_numeric_date("03/04/99").is_none());
}

#[test]
fn german_month_span_offsets_are_character_indices() {
    let value = serde_json::json!("2026-03-03");
    let source = "am März 3, 2026";
    let (start, end) = match_date(source, &value, &VerifyConfig::default())
        .expect("a German month-name span must match");
    let span: String = source.chars().skip(start).take(end - start).collect();
    assert_eq!(span, "März 3, 2026");
}
