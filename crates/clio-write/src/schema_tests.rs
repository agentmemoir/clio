// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0

use super::{ExtractiveKind, FieldSpec, default_event_fields, json_path};
use serde_json::json;

#[test]
fn default_fields_cover_entity_amount_date() {
    let fields = default_event_fields();
    assert_eq!(fields.len(), 3);
    assert_eq!(fields[0].kind, ExtractiveKind::Entity);
    assert!(fields[0].required);
}

#[test]
fn json_path_reads_nested() {
    let v = json!({"a": {"b": 1}});
    assert_eq!(
        json_path(&v, "a.b").and_then(serde_json::Value::as_i64),
        Some(1)
    );
    assert!(json_path(&v, "a.c").is_none());
    let _ = FieldSpec::new("x", ExtractiveKind::Number, false);
}
