// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `inspect` human text view: every field the JSON
//! payload carries appears in the text form, from the response metadata
//! through each row field to the supersession section.
//!
//! ## Owns
//! - Field-by-field parity assertions over the `inspect` text view
//! - The row, metadata, freshness, and supersession line formats
//!
//! ## Does not own
//! - The view under test (`cli_read_render_inspect`)
//! - The inspect tool semantics and row shape (see `clio-mcp` / `clio-store`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::cli_read_render_inspect::render_inspect;
use serde_json::{Value, json};

/// A row carrying every field the tool serializes, plus `context` and
/// `freshness`, so parity is asserted over each field name and value.
fn row() -> Value {
    json!({
        "item_id": "itm-1",
        "kind": "semantic",
        "category": "profile",
        "episodic_type": null,
        "epistemic_kind": "fact",
        "source_type": "user_stated",
        "confidence": 0.9,
        "admission_score": 0.8,
        "source_ref": "ev-1",
        "valid_from": "2026-01-01T00:00:00Z",
        "valid_until": null,
        "tx_from": "2026-01-01T00:00:00Z",
        "tx_until": null,
        "discarded": false,
        "archived": false,
        "created_at": "2026-01-01T00:00:00Z",
        "updated_at": "2026-01-02T00:00:00Z",
        "context": "the user's saved note",
        "freshness": {
            "revision": 2,
            "proof_count": 3,
            "last_reconciled_at": "2026-01-03T00:00:00Z",
            "stale": false,
            "stale_reason": null,
            "evidence": [{"source_id": "src-1", "as_of": "2026-01-03T00:00:00Z"}]
        }
    })
}

/// The representative payload: metadata, one full row, and the optional
/// supersession section, so every JSON field is present.
fn payload() -> Value {
    json!({
        "items": [row()],
        "total": 5,
        "limit": 50,
        "offset": 0,
        "include_discarded": false,
        "truncated": true,
        "supersession": {
            "verdict": "superseded",
            "query_item_id": "itm-1",
            "replacement_item_id": "itm-2",
            "superseded_item_id": "itm-1",
            "page_size": 2,
            "page_limit": 10,
            "reads": ["audit_trail", "retrieve"],
            "reranked": false
        }
    })
}

/// Every JSON field of the payload appears in the text view, field by field.
#[test]
fn text_carries_every_field_the_json_payload_carries() {
    let payload = payload();
    let text = render_inspect(&payload);
    for key in payload.as_object().expect("object").keys() {
        assert!(text.contains(key), "{key} missing from text: {text}");
    }
    // The row's `item_id` is the row head itself (the bare id line), so it is
    // not re-named as a field; every other row field is named.
    for key in row()
        .as_object()
        .expect("row object")
        .keys()
        .filter(|key| key.as_str() != "item_id")
    {
        assert!(text.contains(key), "{key} missing from text: {text}");
    }
    for key in [
        "verdict",
        "query_item_id",
        "replacement_item_id",
        "superseded_item_id",
        "page_size",
        "page_limit",
        "reads",
        "reranked",
        "revision",
        "proof_count",
        "last_reconciled_at",
        "stale",
        "stale_reason",
        "evidence",
    ] {
        assert!(text.contains(key), "{key} missing from text: {text}");
    }
}

/// The rendered view's exact shape for the representative payload, so the
/// format is locked, not just parity-checked. Nested object fields print in
/// the payload's key order, and a nested array prints as its compact JSON.
#[test]
fn the_view_prints_metadata_rows_and_supersession_in_order() {
    let freshness = concat!(
        "  freshness: evidence [{\"as_of\":\"2026-01-03T00:00:00Z\",\"source_id\":\"src-1\"}], ",
        "last_reconciled_at 2026-01-03T00:00:00Z, proof_count 3, revision 2, ",
        "stale false, stale_reason null"
    );
    let supersession = concat!(
        "supersession: page_limit 10, page_size 2, query_item_id itm-1, ",
        "reads [\"audit_trail\",\"retrieve\"], replacement_item_id itm-2, ",
        "reranked false, superseded_item_id itm-1, verdict superseded"
    );
    let lines = [
        "items: 1 of 5",
        "total: 5",
        "limit: 50",
        "offset: 0",
        "include_discarded: false",
        "truncated: true",
        "itm-1",
        "  kind: semantic",
        "  category: profile",
        "  episodic_type: null",
        "  epistemic_kind: fact",
        "  source_type: user_stated",
        "  confidence: 0.9",
        "  admission_score: 0.8",
        "  source_ref: ev-1",
        "  valid_from: 2026-01-01T00:00:00Z",
        "  valid_until: null",
        "  tx_from: 2026-01-01T00:00:00Z",
        "  tx_until: null",
        "  discarded: false",
        "  archived: false",
        "  created_at: 2026-01-01T00:00:00Z",
        "  updated_at: 2026-01-02T00:00:00Z",
        "  context: the user's saved note",
        freshness,
        supersession,
    ];
    assert_eq!(render_inspect(&payload()), lines.join("\n"));
}

/// A clamped page names the served limit and the requested one in the header.
#[test]
fn a_clamped_page_names_both_limits_in_the_header() {
    let text = render_inspect(&json!({
        "items": [],
        "total": 9,
        "limit": 200,
        "requested_limit": 201
    }));
    assert!(
        text.contains("items: 0 of 9 (limit 200, lowered from 201)"),
        "{text}"
    );
    assert!(text.contains("total: 9"), "{text}");
    assert!(text.contains("limit: 200"), "{text}");
}

/// An empty payload keeps the bare header: metadata the payload does not
/// carry adds no line.
#[test]
fn an_empty_payload_keeps_the_bare_header() {
    assert_eq!(render_inspect(&json!({})), "items: 0 of 0");
}

/// A field a row carries outside the known shape still gets a line, so a
/// field the payload adds later cannot silently disappear.
#[test]
fn an_unknown_row_field_still_gets_a_line() {
    let text = render_inspect(&json!({"items": [{"item_id": "itm-1", "new_field": 7}]}));
    assert!(text.contains("new_field: 7"), "{text}");
}
