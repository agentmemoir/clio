// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `stats` human text view: every field the JSON report
//! carries appears in the text form, including every breakdown group, its
//! bucket labels, and its counts.
//!
//! ## Owns
//! - Field-by-field parity assertions over the `stats` text view
//! - The empty-group and missing-field wordings
//!
//! ## Does not own
//! - The view under test (`cli_read_render_stats`)
//! - Stats computation and report shape (see `clio-compliance`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::cli_read_render_stats::render_stats;
use serde_json::{Value, json};

fn buckets(label: &str, count: u64) -> Value {
    json!([{"label": label, "count": count}])
}

/// A representative report with every group populated, so parity is asserted
/// over each field, each label, and each count.
fn report() -> Value {
    json!({
        "bank": "bank-a",
        "items_total": 10,
        "items_active": 8,
        "items_discarded": 2,
        "by_tier": [{"label": "semantic", "count": 3}, {"label": "episodic", "count": 5}],
        "by_category": buckets("profile", 2),
        "by_epistemic_kind": buckets("fact", 4),
        "by_age_band": buckets("0-1 half-lives", 6),
        "by_provenance_class": buckets("user_stated", 3),
        "pressure_counts": buckets("flagged", 1)
    })
}

/// Every JSON field of the report appears in the text view, field by field,
/// together with every bucket label and count the groups carry.
#[test]
fn text_carries_every_field_the_json_report_carries() {
    let payload = report();
    let text = render_stats(&payload);
    for key in payload.as_object().expect("object").keys() {
        assert!(text.contains(key), "{key} missing from text: {text}");
    }
    assert_eq!(
        text,
        "bank: bank-a\n\
         items_total: 10\n\
         items_active: 8\n\
         items_discarded: 2\n\
         by_tier: semantic 3, episodic 5\n\
         by_category: profile 2\n\
         by_epistemic_kind: fact 4\n\
         by_age_band: 0-1 half-lives 6\n\
         by_provenance_class: user_stated 3\n\
         pressure_counts: flagged 1"
    );
}

/// An empty group is named `none`, not dropped, so the text still names the
/// field the JSON carries.
#[test]
fn an_empty_group_is_named_not_dropped() {
    let payload = json!({
        "bank": "bank-a",
        "items_total": 0,
        "items_active": 0,
        "items_discarded": 0,
        "by_tier": [],
        "by_category": [],
        "by_epistemic_kind": [],
        "by_age_band": [],
        "by_provenance_class": [],
        "pressure_counts": []
    });
    let text = render_stats(&payload);
    for key in [
        "by_tier",
        "by_category",
        "by_epistemic_kind",
        "by_age_band",
        "by_provenance_class",
        "pressure_counts",
    ] {
        assert!(text.contains(&format!("{key}: none")), "{key}: {text}");
    }
}

/// A field the payload does not carry prints `?`, never a fabricated zero.
#[test]
fn a_missing_field_prints_a_placeholder() {
    let text = render_stats(&json!({}));
    assert!(text.contains("bank: ?"), "{text}");
    assert!(text.contains("items_total: ?"), "{text}");
    assert!(text.contains("by_tier: ?"), "{text}");
}
