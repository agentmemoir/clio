// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0

use crate::schema::{ExtractiveKind, FieldSpec, default_event_fields};
use crate::verify::{VerifyConfig, nfc, verify_snapshot};
use serde_json::json;

fn entity_amount_date() -> Vec<FieldSpec> {
    default_event_fields()
}

#[test]
fn t01_grounded_entities_numbers_dates_pass() {
    let source = "Ada met 1,000 users on 2026-04-03";
    let snap = json!({
        "entity": "Ada",
        "amount": 1000,
        "date": "2026-04-03"
    });
    let report = verify_snapshot(
        source,
        &snap,
        &entity_amount_date(),
        &VerifyConfig::default(),
    );
    assert!(report.ok, "{report:?}");
    assert_eq!(report.spans.len(), 3);
}

#[test]
fn t02_paraphrase_entity_fails() {
    let source = "Ada prefers concise replies";
    let snap = json!({"entity": "The user"});
    let fields = [FieldSpec::new("entity", ExtractiveKind::Entity, true)];
    let report = verify_snapshot(source, &snap, &fields, &VerifyConfig::default());
    assert!(!report.ok);
    assert!(report.failures[0].reason.contains("not found"));
}

#[test]
fn t03_number_format_variant_with_span() {
    let source = "saw 1,000 users today";
    let snap = json!({"amount": 1000});
    let fields = [FieldSpec::new("amount", ExtractiveKind::Number, true)];
    let report = verify_snapshot(source, &snap, &fields, &VerifyConfig::default());
    assert!(report.ok, "{report:?}");
    let span = &report.spans[0];
    let norm = nfc(source);
    let chars: Vec<char> = norm.chars().collect();
    let piece: String = chars[span.start..span.end].iter().collect();
    assert!(piece.contains('1') && piece.contains('0'));
}

#[test]
fn t03b_number_without_digit_span_fails() {
    let source = "about a thousand users";
    let snap = json!({"amount": 1000});
    let fields = [FieldSpec::new("amount", ExtractiveKind::Number, true)];
    let report = verify_snapshot(source, &snap, &fields, &VerifyConfig::default());
    assert!(!report.ok);
}

#[test]
fn t04_ambiguous_date_without_day_first_fails() {
    let source = "meeting on 03/04/2026";
    let snap = json!({"date": "2026-04-03"});
    let fields = [FieldSpec::new("date", ExtractiveKind::Date, true)];
    let report = verify_snapshot(source, &snap, &fields, &VerifyConfig::default());
    assert!(!report.ok);
    assert!(
        report.failures[0].reason.contains("ambiguous"),
        "{report:?}"
    );
}

#[test]
fn ambiguous_date_passes_with_day_first() {
    let source = "meeting on 03/04/2026";
    let snap = json!({"date": "2026-04-03"});
    let fields = [FieldSpec::new("date", ExtractiveKind::Date, true)];
    let cfg = VerifyConfig {
        day_first: Some(true),
        ..VerifyConfig::default()
    };
    let report = verify_snapshot(source, &snap, &fields, &cfg);
    assert!(report.ok, "{report:?}");
}

#[test]
fn t10_deterministic_twice() {
    let source = "Ada paid $2.5";
    let snap = json!({"entity": "Ada", "amount": 2.5});
    let fields = [
        FieldSpec::new("entity", ExtractiveKind::Entity, true),
        FieldSpec::new("amount", ExtractiveKind::Number, true),
    ];
    let a = verify_snapshot(source, &snap, &fields, &VerifyConfig::default());
    let b = verify_snapshot(source, &snap, &fields, &VerifyConfig::default());
    assert_eq!(a, b);
    assert!(a.ok);
}

#[test]
fn missing_required_field_fails() {
    let source = "Ada";
    let snap = json!({});
    let fields = [FieldSpec::new("entity", ExtractiveKind::Entity, true)];
    let report = verify_snapshot(source, &snap, &fields, &VerifyConfig::default());
    assert!(!report.ok);
}

#[test]
fn optional_absent_field_skips() {
    let source = "Ada";
    let snap = json!({"entity": "Ada"});
    let report = verify_snapshot(
        source,
        &snap,
        &entity_amount_date(),
        &VerifyConfig::default(),
    );
    assert!(report.ok, "{report:?}");
}

#[test]
fn unlisted_top_level_leaf_rejected() {
    let source = "Ada met 1,000 users on 2026-04-03";
    let snap = json!({
        "entity": "Ada",
        "amount": 1000,
        "invented_note": "unverified paraphrase"
    });
    let report = verify_snapshot(
        source,
        &snap,
        &entity_amount_date(),
        &VerifyConfig::default(),
    );
    assert!(!report.ok);
    assert_eq!(report.failures.len(), 1, "{report:?}");
    assert_eq!(report.failures[0].field, "<unlisted leaf #0>");
    assert!(report.failures[0].reason.contains("unlisted snapshot leaf"));
    assert!(
        !report
            .spans
            .iter()
            .any(|s| s.field != "entity" && s.field != "amount"),
        "{report:?}"
    );
}

#[test]
fn unlisted_nested_leaf_rejected() {
    let source = "Ada met 1,000 users on 2026-04-03";
    let snap = json!({
        "entity": "Ada",
        "invented": {"deep": "unverified", "empty": "", "nul": null}
    });
    let report = verify_snapshot(
        source,
        &snap,
        &entity_amount_date(),
        &VerifyConfig::default(),
    );
    assert!(!report.ok);
    assert_eq!(report.failures.len(), 1, "{report:?}");
    // The nested path is not echoed; only the walk position is reported.
    assert_eq!(report.failures[0].field, "<unlisted leaf #0>");
    // A nested leaf inside a declared field is also unlisted: the declared
    // path owns one leaf value, not a subtree of unverified leaves.
    let snap = json!({"entity": {"name": "Ada"}});
    let report = verify_snapshot(
        source,
        &snap,
        &entity_amount_date(),
        &VerifyConfig::default(),
    );
    assert!(!report.ok);
    assert!(
        report
            .failures
            .iter()
            .any(|f| f.reason.contains("unlisted") && f.field.starts_with("<unlisted leaf #")),
        "{report:?}"
    );
}

#[test]
fn unlisted_list_leaf_rejected() {
    let source = "Ada met 1,000 users on 2026-04-03";
    let snap = json!({
        "entity": "Ada",
        "invented_list": ["a", "b"]
    });
    let report = verify_snapshot(
        source,
        &snap,
        &entity_amount_date(),
        &VerifyConfig::default(),
    );
    assert!(!report.ok);
    let fields: Vec<&str> = report.failures.iter().map(|f| f.field.as_str()).collect();
    assert_eq!(
        fields,
        vec!["<unlisted leaf #0>", "<unlisted leaf #1>"],
        "{report:?}"
    );
    // A list under a declared path is also unlisted: the declared policy is
    // typed for a scalar, never a subtree.
    let snap = json!({"entity": "Ada", "amount": [10]});
    let report = verify_snapshot(
        source,
        &snap,
        &entity_amount_date(),
        &VerifyConfig::default(),
    );
    assert!(!report.ok);
    assert!(
        report
            .failures
            .iter()
            .any(|f| f.reason.contains("unlisted")),
        "{report:?}"
    );
}

#[test]
fn unlisted_leaf_failure_never_echoes_caller_key() {
    // A snapshot key is caller-controlled, so it may itself be the secret.
    let source = "Ada met 1,000 users on 2026-04-03";
    let secret_key = "sk-live-LEAKEDKEY99";
    let snap = json!({
        "entity": "Ada",
        secret_key: "x"
    });
    let report = verify_snapshot(
        source,
        &snap,
        &entity_amount_date(),
        &VerifyConfig::default(),
    );
    assert!(!report.ok);
    let serialized = serde_json::to_string(&report).expect("report serializes");
    assert!(
        !serialized.contains(secret_key),
        "caller key must not reach the report: {serialized}"
    );
    assert!(
        !serialized.contains("sk-live"),
        "no fragment of the caller key may reach the report: {serialized}"
    );
}

#[test]
fn empty_unlisted_leaves_ignored() {
    let source = "Ada met 1,000 users on 2026-04-03";
    let snap = json!({
        "entity": "Ada",
        "amount": 1000,
        "note": "",
        "blank": "   ",
        "nul": null,
        "empty_obj": {},
        "empty_list": []
    });
    let report = verify_snapshot(
        source,
        &snap,
        &entity_amount_date(),
        &VerifyConfig::default(),
    );
    assert!(report.ok, "{report:?}");
}

#[test]
fn non_object_snapshot_fails() {
    let report = verify_snapshot(
        "Ada",
        &json!("not an object"),
        &entity_amount_date(),
        &VerifyConfig::default(),
    );
    assert!(!report.ok);
    assert_eq!(report.failures[0].field, "<snapshot>");
    assert!(report.failures[0].reason.contains("not an object"));
    let report = verify_snapshot(
        "Ada",
        &json!([1]),
        &entity_amount_date(),
        &VerifyConfig::default(),
    );
    assert!(!report.ok);
    assert_eq!(report.failures[0].field, "<snapshot>");
}

#[test]
fn mixed_declared_and_unlisted_failures() {
    let source = "Ada prefers concise replies";
    let snap = json!({
        "entity": "The user",
        "amount": 1000,
        "invented_note": "unverified"
    });
    let report = verify_snapshot(
        source,
        &snap,
        &entity_amount_date(),
        &VerifyConfig::default(),
    );
    assert!(!report.ok);
    let fields: Vec<&str> = report.failures.iter().map(|f| f.field.as_str()).collect();
    // Declared paths are static policy text and stay nameable; the unlisted
    // leaf is reported by position only.
    assert!(fields.contains(&"entity"), "{report:?}");
    assert!(fields.contains(&"amount"), "{report:?}");
    assert!(fields.contains(&"<unlisted leaf #0>"), "{report:?}");
    assert!(!fields.contains(&"invented_note"), "{report:?}");
}
