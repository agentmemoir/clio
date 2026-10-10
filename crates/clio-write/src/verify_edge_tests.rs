// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//! Edge cases for entity/number/date matchers (coverage + correctness).

use crate::schema::{ExtractiveKind, FieldSpec};
use crate::verify::{VerifyConfig, verify_snapshot};
use crate::verify_entity::match_entity;
use crate::verify_number::match_number;
use serde_json::json;

#[test]
fn entity_rejects_non_string_empty_and_too_long() {
    assert!(match_entity("Ada", &json!(1)).is_err());
    assert!(match_entity("Ada", &json!("   ")).is_err());
    assert!(match_entity("A", &json!("Ada")).is_err());
}

#[test]
fn entity_nfc_trim_match() {
    let source = "  café ";
    let nfc_source: String = unicode_normalization::UnicodeNormalization::nfc(source).collect();
    let (s, e) = match_entity(&nfc_source, &json!("café")).unwrap();
    assert!(e > s);
}

#[test]
fn number_string_currency_and_eu_locale() {
    let cfg = VerifyConfig {
        comma_grouping: false,
        ..VerifyConfig::default()
    };
    let (s, e) = match_number("price 1.234,56 €", &json!("1234.56"), &cfg).unwrap();
    assert!(e > s);
    let (s, e) = match_number("cost $42", &json!(42), &VerifyConfig::default()).unwrap();
    assert!(e > s);
}

#[test]
fn number_rejects_bool_and_bad_string() {
    assert!(match_number("1", &json!(true), &VerifyConfig::default()).is_err());
    assert!(match_number("1", &json!("nope"), &VerifyConfig::default()).is_err());
}

#[test]
fn number_range_finds_either_endpoint() {
    let cfg = VerifyConfig::default();
    assert!(match_number("took 10 - 15 minutes", &json!(10), &cfg).is_ok());
    assert!(match_number("took 10 - 15 minutes", &json!(15), &cfg).is_ok());
}

#[test]
fn datetime_precision_rejects_hallucinated_time() {
    let fields = [FieldSpec::new("date", ExtractiveKind::Date, true)];
    // Snapshot has a time; source is date-only → fail.
    let report = verify_snapshot(
        "met on 2026-04-03",
        &json!({"date": "2026-04-03T14:30:00"}),
        &fields,
        &VerifyConfig::default(),
    );
    assert!(!report.ok, "{report:?}");
    // Same calendar day but different time → fail.
    let report = verify_snapshot(
        "met on 2026-04-03T01:00:00",
        &json!({"date": "2026-04-03T14:30:00"}),
        &fields,
        &VerifyConfig::default(),
    );
    assert!(!report.ok, "{report:?}");
}

#[test]
fn utf8_month_name_does_not_panic_and_matches() {
    let fields = [FieldSpec::new("date", ExtractiveKind::Date, true)];
    let report = verify_snapshot(
        "geliefert März 3, 2026",
        &json!({"date": "2026-03-03"}),
        &fields,
        &VerifyConfig::default(),
    );
    assert!(report.ok, "{report:?}");
}

#[test]
fn date_month_name_and_forced_day_month() {
    let fields = [FieldSpec::new("date", ExtractiveKind::Date, true)];
    let report = verify_snapshot(
        "shipped Apr 3, 2026",
        &json!({"date": "2026-04-03"}),
        &fields,
        &VerifyConfig::default(),
    );
    assert!(report.ok, "{report:?}");

    let report = verify_snapshot(
        "event on 25/04/2026",
        &json!({"date": "2026-04-25"}),
        &fields,
        &VerifyConfig::default(),
    );
    assert!(report.ok, "{report:?}");

    let report = verify_snapshot(
        "event on 04/25/2026",
        &json!({"date": "2026-04-25"}),
        &fields,
        &VerifyConfig::default(),
    );
    assert!(report.ok, "{report:?}");
}

#[test]
fn date_iso_datetime_snapshot_and_month_first_policy() {
    let fields = [FieldSpec::new("date", ExtractiveKind::Date, true)];
    let report = verify_snapshot(
        "at 2026-04-03T10:00:00",
        &json!({"date": "2026-04-03T10:00:00"}),
        &fields,
        &VerifyConfig::default(),
    );
    assert!(report.ok, "{report:?}");

    let cfg = VerifyConfig {
        day_first: Some(false),
        ..VerifyConfig::default()
    };
    let report = verify_snapshot(
        "on 03/04/2026",
        &json!({"date": "2026-03-04"}),
        &fields,
        &cfg,
    );
    assert!(report.ok, "{report:?}");
}

#[test]
fn date_rejects_non_string_empty_and_bad_iso() {
    let fields = [FieldSpec::new("date", ExtractiveKind::Date, true)];
    assert!(!verify_snapshot("x", &json!({"date": 1}), &fields, &VerifyConfig::default()).ok);
    assert!(!verify_snapshot("x", &json!({"date": ""}), &fields, &VerifyConfig::default()).ok);
    assert!(
        !verify_snapshot(
            "x",
            &json!({"date": "not-a-date"}),
            &fields,
            &VerifyConfig::default()
        )
        .ok
    );
}

#[test]
fn date_dotted_and_two_digit_year_ignored() {
    let fields = [FieldSpec::new("date", ExtractiveKind::Date, true)];
    let report = verify_snapshot(
        "on 03.04.2026",
        &json!({"date": "2026-04-03"}),
        &fields,
        &VerifyConfig {
            day_first: Some(true),
            ..VerifyConfig::default()
        },
    );
    assert!(report.ok, "{report:?}");
    // Two-digit year must not match.
    let report = verify_snapshot(
        "on 03/04/26",
        &json!({"date": "2026-04-03"}),
        &fields,
        &VerifyConfig {
            day_first: Some(true),
            ..VerifyConfig::default()
        },
    );
    assert!(!report.ok);
}
