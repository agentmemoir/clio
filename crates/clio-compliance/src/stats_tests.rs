// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the read-only `stats` reporter's
//! count accuracy and bank isolation.
//!
//! ## Owns
//! - Count-accuracy and bank-scoping assertions for `stats`, including age
//!   bands, provenance classes, and relayed pressure counts
//! - Collision-free identifiers for isolated test stores
//!
//! ## Does not own
//! - The stats aggregation itself (see `stats.rs`)
//! - MCP tool binding for `stats` (see `clio-mcp`)
//!
//! ## Boundary
//!
//! Changes to stats count or isolation coverage should normally require changes here.
//! Changes to stats aggregation or MCP binding should not require changes here.
//!
//! Keep this module focused on stats reporter tests.

use crate::stats::{Count, PressureCounts, StatsInputs, stats};
use crate::test_unique::unique;
use clio_store::{DekProvider, LocalDevKms, SqliteStore, Store};
use clio_types::{
    EpisodicType, EpistemicKind, ErrorCode, ItemKind, MemoryItem, SemanticCategory, SourceType,
};
use std::sync::Arc;

fn store() -> SqliteStore {
    let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
    SqliteStore::open_in_memory(kms).unwrap()
}

fn item(id: &str, bank: &str, subject: &str, category: Option<SemanticCategory>) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: subject.into(),
        kind: if category.is_some() {
            ItemKind::Semantic
        } else {
            ItemKind::Episodic
        },
        category,
        episodic_type: category.is_none().then_some(EpisodicType::Gist),
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.9),
        admission_score: Some(0.8),
        source_ref: None,
        snapshot: Some(serde_json::json!({"k": "v"})),
        gist: Some("gist".into()),
        valid_from: "2026-01-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2026-01-01T00:00:00Z".into(),
        tx_until: None,
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-01-01T00:00:00Z".into(),
        context: None,
    }
}

/// An item variant stamped with a creation instant (the store persists
/// `created_at` verbatim).
fn with_created_at(item: MemoryItem, created_at: &str) -> MemoryItem {
    let mut item = item;
    item.created_at = created_at.into();
    item
}

/// An item variant that is a belief of one provenance rung.
fn with_source(item: MemoryItem, source: SourceType) -> MemoryItem {
    let mut item = item;
    item.epistemic_kind = EpistemicKind::Belief;
    item.source_type = Some(source);
    item
}

/// Default caller inputs: the default half-life (30 days) resolved
/// caller-side, one fixed instant, no pressure section.
fn inputs(now: &str) -> StatsInputs {
    StatsInputs {
        band_unit_days: 30.0,
        now: now.to_owned(),
        pressure: None,
    }
}

fn count_label(section: &[Count], label: &str) -> usize {
    section
        .iter()
        .find(|c| c.label == label)
        .map_or(0, |c| c.count)
}

fn sum_counts(section: &[Count]) -> usize {
    section.iter().map(|c| c.count).sum()
}

#[test]
fn counts_reflect_created_items_per_bank() {
    let store = store();
    let bank = unique("b");
    let other = unique("b");
    store
        .create_memory_item(
            &item(
                "itm-a",
                &bank,
                "s1",
                Some(SemanticCategory::parse("schema").unwrap()),
            ),
            "tester",
        )
        .unwrap();
    store
        .create_memory_item(
            &item(
                "itm-b",
                &bank,
                "s2",
                Some(SemanticCategory::parse("persona").unwrap()),
            ),
            "tester",
        )
        .unwrap();
    store
        .create_memory_item(&item("itm-c", &bank, "s3", None), "tester")
        .unwrap();
    // A different bank must not leak into the report.
    store
        .create_memory_item(
            &item(
                "itm-x",
                &other,
                "s4",
                Some(SemanticCategory::parse("schema").unwrap()),
            ),
            "tester",
        )
        .unwrap();

    let report = stats(&store, &bank, &inputs("2026-03-01T00:00:00Z")).unwrap();
    assert_eq!(report.bank, bank);
    assert_eq!(report.items_total, 3);
    assert_eq!(report.items_active, 3);
    assert_eq!(report.items_discarded, 0);
    let semantic = count_label(&report.by_tier, "semantic");
    let episodic = count_label(&report.by_tier, "episodic");
    assert_eq!(semantic + episodic, report.items_active);
    assert_eq!(semantic, 2);
    assert_eq!(episodic, 1);
    let schema = count_label(&report.by_category, "schema");
    let persona = count_label(&report.by_category, "persona");
    assert_eq!(schema, 1);
    assert_eq!(persona, 1);
}

#[test]
fn discard_counts_and_other_bank_isolation() {
    let store = store();
    let bank = unique("b");
    let subject = unique("s");
    store
        .create_memory_item(
            &item(
                "itm-1",
                &bank,
                &subject,
                Some(SemanticCategory::parse("tool_config").unwrap()),
            ),
            "tester",
        )
        .unwrap();
    store
        .discard_item(&bank, "itm-1", "noise", "2026-01-02T00:00:00Z", "tester")
        .unwrap();
    let report = stats(&store, &bank, &inputs("2026-03-01T00:00:00Z")).unwrap();
    assert_eq!(report.items_total, 1);
    assert_eq!(report.items_active, 0);
    assert_eq!(report.items_discarded, 1);
    assert_eq!(count_label(&report.by_tier, "semantic"), 0);
    assert_eq!(count_label(&report.by_tier, "episodic"), 0);
}

#[test]
fn empty_bank_reports_zeroes() {
    let store = store();
    let bank = unique("empty");
    let report = stats(&store, &bank, &inputs("2026-03-01T00:00:00Z")).unwrap();
    assert_eq!(report.items_total, 0);
    assert_eq!(report.items_active, 0);
    assert_eq!(report.by_category.len(), 5);
    assert!(report.by_category.iter().all(|c| c.count == 0));
    // Empty banks still carry every fixed-size list entry.
    assert_eq!(report.by_age_band.len(), 4);
    assert!(report.by_age_band.iter().all(|c| c.count == 0));
    assert_eq!(report.by_provenance_class.len(), 4);
    assert!(report.by_provenance_class.iter().all(|c| c.count == 0));
    assert_eq!(report.pressure_counts, [] as [Count; 0]);
}

#[test]
fn empty_bank_id_rejected() {
    let store = store();
    let err = stats(&store, "", &inputs("2026-03-01T00:00:00Z")).unwrap_err();
    assert_eq!(err.code, ErrorCode::MissingBank);
}

#[test]
fn age_bands_follow_the_resolved_half_life_unit() {
    // Unit 7 days, now 2026-03-01: edges at Feb 22 (7d), Feb 15 (14d), and
    // Feb 1 (28d). Ages 4, 12, 19, and 55 days fall one per band.
    let store = store();
    let bank = unique("b");
    let subject = unique("s");
    for (id, stamp) in [
        ("itm-y", "2026-02-25T00:00:00Z"),
        ("itm-m", "2026-02-17T00:00:00Z"),
        ("itm-o", "2026-02-10T00:00:00Z"),
        ("itm-e", "2026-01-05T00:00:00Z"),
    ] {
        store
            .create_memory_item(
                &with_created_at(item(id, &bank, &subject, None), stamp),
                "tester",
            )
            .unwrap();
    }
    let inputs = StatsInputs {
        band_unit_days: 7.0,
        now: "2026-03-01T00:00:00Z".into(),
        pressure: None,
    };
    let report = stats(&store, &bank, &inputs).unwrap();
    let counts: Vec<_> = report.by_age_band.iter().map(|c| c.count).collect();
    assert_eq!(counts, vec![1, 1, 1, 1], "{:?}", report.by_age_band);
    assert_eq!(report.by_age_band.len(), 4);
    assert_eq!(report.by_age_band[0].label, "0–1 half-life (0–7 days)");
    // "half-li" is the stem of both inflections: "half-life" and "half-lives".
    assert!(
        report
            .by_age_band
            .iter()
            .all(|c| c.label.contains("half-li"))
    );
    assert_eq!(sum_counts(&report.by_age_band), report.items_active);
}

#[test]
fn item_created_exactly_on_a_band_edge_counts_in_the_younger_band() {
    // Unit 30 days, now 2026-03-01: the 1-half-life edge is 2026-01-30, so an
    // item created exactly there has age exactly 30 days and belongs to the
    // younger band.
    let store = store();
    let bank = unique("b");
    let subject = unique("s");
    store
        .create_memory_item(
            &with_created_at(
                item("itm-edge", &bank, &subject, None),
                "2026-01-30T00:00:00Z",
            ),
            "tester",
        )
        .unwrap();
    let report = stats(&store, &bank, &inputs("2026-03-01T00:00:00Z")).unwrap();
    assert_eq!(
        count_label(&report.by_age_band, "0–1 half-life (0–30 days)"),
        1
    );
    assert_eq!(sum_counts(&report.by_age_band), report.items_active);
}

#[test]
fn direct_stats_call_with_literal_unit_reports_bands_with_stated_unit() {
    // This is a direct `stats()` call with a literal `band_unit_days` (30, the
    // default half-life); it exercises the band computation and the stated
    // unit, not the profile's `retention_curve` flag — the disabled-curve path
    // is covered end to end in `crates/clio-mcp`'s stats disabled-curve test.
    let store = store();
    let bank = unique("b");
    let subject = unique("s");
    for (id, stamp) in [
        ("itm-new", "2026-02-25T00:00:00Z"), // age 4 days
        ("itm-old", "2025-06-01T00:00:00Z"), // age 273 days
    ] {
        store
            .create_memory_item(
                &with_created_at(item(id, &bank, &subject, None), stamp),
                "tester",
            )
            .unwrap();
    }
    let report = stats(&store, &bank, &inputs("2026-03-01T00:00:00Z")).unwrap();
    assert_eq!(
        count_label(&report.by_age_band, "0–1 half-life (0–30 days)"),
        1
    );
    assert_eq!(
        count_label(&report.by_age_band, "4+ half-lives (120+ days)"),
        1
    );
    assert_eq!(sum_counts(&report.by_age_band), report.items_active);
    // "half-li" is the stem of both inflections: "half-life" and "half-lives".
    assert!(
        report
            .by_age_band
            .iter()
            .all(|c| c.label.contains("half-li"))
    );
}

#[test]
fn provenance_classes_hand_computed_and_sum_to_active() {
    let store = store();
    let bank = unique("b");
    let subject = unique("s");
    store
        .create_memory_item(&item("itm-f", &bank, &subject, None), "tester")
        .unwrap();
    store
        .create_memory_item(
            &with_source(item("itm-u", &bank, &subject, None), SourceType::UserStated),
            "tester",
        )
        .unwrap();
    store
        .create_memory_item(
            &with_source(
                item("itm-i", &bank, &subject, None),
                SourceType::AgentInferred,
            ),
            "tester",
        )
        .unwrap();
    store
        .create_memory_item(
            &with_source(item("itm-t", &bank, &subject, None), SourceType::ThirdParty),
            "tester",
        )
        .unwrap();
    // A discarded belief must not count: the classes cover active rows only.
    store
        .create_memory_item(
            &with_source(item("itm-x", &bank, &subject, None), SourceType::UserStated),
            "tester",
        )
        .unwrap();
    store
        .discard_item(&bank, "itm-x", "noise", "2026-03-02T00:00:00Z", "tester")
        .unwrap();

    let report = stats(&store, &bank, &inputs("2026-03-01T00:00:00Z")).unwrap();
    assert_eq!(count_label(&report.by_provenance_class, "user_stated"), 1);
    assert_eq!(
        count_label(&report.by_provenance_class, "agent_inferred"),
        1
    );
    assert_eq!(count_label(&report.by_provenance_class, "third_party"), 1);
    // The fact rung is the stored NULL population, labelled "fact".
    assert_eq!(count_label(&report.by_provenance_class, "fact"), 1);
    assert_eq!(report.by_provenance_class.len(), 4);
    assert_eq!(sum_counts(&report.by_provenance_class), report.items_active);
}

#[test]
fn pressure_counts_relay_only_present_values() {
    let store = store();
    let bank = unique("b");
    let pressured = StatsInputs {
        band_unit_days: 30.0,
        now: "2026-03-01T00:00:00Z".into(),
        pressure: Some(PressureCounts {
            flagged: Some(3),
            stale_summaries: None,
            archive_candidates: Some(7),
        }),
    };
    let report = stats(&store, &bank, &pressured).unwrap();
    assert_eq!(
        report.pressure_counts,
        vec![
            Count {
                label: "flagged".into(),
                count: 3,
            },
            Count {
                label: "archive candidates".into(),
                count: 7,
            },
        ]
    );
    // No pressure input at all: an empty list, never a fabricated zero.
    let none_report = stats(&store, &bank, &inputs("2026-03-01T00:00:00Z")).unwrap();
    assert_eq!(none_report.pressure_counts, [] as [Count; 0]);
}

#[test]
fn invalid_now_stamp_rejected() {
    let store = store();
    let bank = unique("b");
    let inputs = StatsInputs {
        band_unit_days: 30.0,
        now: "not-a-stamp".into(),
        pressure: None,
    };
    let err = stats(&store, &bank, &inputs).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
}

#[test]
fn tier_doc_states_semantic_versus_episodic_split() {
    // The tier definition lives in the `by_tier` documentation in stats.rs
    // (the include_str!-the-source pattern, as in clio-retrieve assoc_tests).
    let source = include_str!("stats.rs");
    assert!(
        source.contains("semantic-versus-episodic split"),
        "by_tier documentation must state the semantic-versus-episodic split"
    );
    assert!(
        source.contains("nothing else on this surface"),
        "by_tier documentation must scope the word tier to this surface"
    );
}
