// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the SQLite `SyncStore` change-feed SQL bodies on an
//! in-memory store: the item/assoc/triple watermark queries, the empty
//! watermark sentinel handling, and the feed page cap.
//!
//! ## Owns
//! - SQLite `SyncStore` change-feed assertions (item / assoc / triple feed)
//!
//! ## Does not own
//! - Journal / cursor / conflict / dead-letter assertions (`sync_store_tests`)
//! - Postgres sync parity (`pg_sync_tests`)
//! - Sync protocol semantics (`clio-sync`)
//!
//! ## Boundary
//!
//! Tests here break when SQLite feed SQL changes; they never change it.
//!
//! Keep this module focused on SQLite sync change-feed tests.

use super::*;
use clio_types::{EpistemicKind, ItemKind, MemoryItem, SemanticCategory};

#[test]
fn feed_items_classifies_upsert_invalidate_discard() {
    let s = store();
    let item = MemoryItem {
        id: "feed-1".into(),
        bank_id: "b1".into(),
        subject_id: "subj".into(),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::ToolConfig),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        admission_score: Some(0.9),
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
    };
    s.create_memory_item(&item, "actor").expect("create");

    let feed = s
        .feed_items("b1", "2025-12-31T00:00:00Z", 10)
        .expect("feed");
    assert_eq!(feed.len(), 1);
    assert_eq!(feed[0].entity_id, "feed-1");
    assert_eq!(feed[0].op, "upsert");

    // Nothing new past a later watermark.
    let later_items = s
        .feed_items("b1", "2030-01-01T00:00:00Z", 10)
        .expect("feed");
    assert!(later_items.is_empty(), "{later_items:?}");
    // Other banks are isolated.
    let other_bank_items = s
        .feed_items("b2", "2025-12-31T00:00:00Z", 10)
        .expect("feed");
    assert!(other_bank_items.is_empty(), "{other_bank_items:?}");

    let _ = s.invalidate_item(
        "b1",
        "feed-1",
        "2026-01-02T00:00:00Z",
        "2026-01-02T00:00:00Z",
        None,
        "actor",
    );
    let feed = s
        .feed_items("b1", "2026-01-01T00:00:01Z", 10)
        .expect("feed");
    assert_eq!(feed[0].op, "invalidate");

    s.discard_item(
        "b1",
        "feed-1",
        "ops cleanup",
        "2026-01-03T00:00:00Z",
        "actor",
    )
    .expect("discard");
    let feed = s
        .feed_items("b1", "2026-01-01T00:00:01Z", 10)
        .expect("feed");
    assert!(feed.iter().any(|r| r.op == "discard"));
}

#[test]
fn feed_assoc_and_triples_respect_watermark() {
    let s = store();
    let item_a = feed_item("edge-a", "b1");
    let item_b = feed_item("edge-b", "b1");
    s.create_memory_item(&item_a, "actor").expect("a");
    s.create_memory_item(&item_b, "actor").expect("b");
    let edge = am_store_assoc(&item_a.id, &item_b.id);
    s.put_assoc_edge(&edge).expect("put edge");
    let feed = s
        .feed_assoc("b1", "2025-12-31T00:00:00Z", 10)
        .expect("feed");
    assert_eq!(feed.len(), 1);
    assert_eq!(feed[0].entity_kind, "assoc_edge");
    let later_assoc = s
        .feed_assoc("b1", "2030-01-01T00:00:00Z", 10)
        .expect("feed");
    assert!(later_assoc.is_empty(), "{later_assoc:?}");

    // Triples appear on the feed once closed on either axis.
    let mut item_c = feed_item("edge-c", "b1");
    item_c.subject_id = "subj-c".into();
    let triple = clio_types::TripleRecord {
        context: None,
        id: "tri-1".into(),
        bank_id: "b1".into(),
        item_id: "edge-c".into(),
        subject: "s".into(),
        predicate: "p".into(),
        object: "o".into(),
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        valid_from: "2026-01-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2026-01-01T00:00:00Z".into(),
        tx_until: None,
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: Some("2026-01-01T00:00:00Z".into()),
        snapshot_ref: None,
        gist_ref: None,
    };
    let outcome = s
        .commit_triple_add(&item_c, &triple, false, "actor")
        .expect("triple add");
    assert_eq!(outcome.new_edge_id, "tri-1");
    let open_triples = s
        .feed_triples("b1", "2026-01-01T00:00:00Z", 10)
        .expect("feed");
    assert!(open_triples.is_empty(), "{open_triples:?}");
    let ended = s
        .triple_end(
            "b1",
            "s",
            "p",
            Some("o"),
            "2026-01-02T00:00:00Z",
            "2026-01-02T00:00:00Z",
        )
        .expect("triple end");
    assert!(!ended.closed_ids.is_empty(), "{:?}", ended.closed_ids);
    let feed = s
        .feed_triples("b1", "2026-01-01T00:00:00Z", 10)
        .expect("feed");
    assert_eq!(feed.len(), 1);
    assert_eq!(feed[0].op, "invalidate");
}

/// The cursor conversion rewrites a retired stamp-only body to the canonical
/// compound form with the same decoded `(stamp, entity)` pair: the split and
/// the feed resume are identical for the two spellings.
#[test]
fn canonical_empty_pair_and_bare_stamp_address_the_same_resume_point() {
    use crate::sync_store_rows::split_push_watermark;
    let s = store();
    let bank = "bpair";
    let stamp = "2026-01-01T00:00:00Z";
    for id in ["i-a", "i-b"] {
        s.create_memory_item(&feed_item(id, bank), "actor")
            .expect("create");
    }
    let bare = stamp.to_owned();
    let canonical = format!("{stamp}\u{1f}");
    assert_eq!(
        split_push_watermark(&bare),
        split_push_watermark(&canonical),
        "the canonical re-encode preserves the decoded (stamp, entity) pair"
    );
    let ids = |since: &str| -> Vec<String> {
        s.feed_items(bank, since, 10)
            .expect("feed")
            .iter()
            .map(|r| r.entity_id.clone())
            .collect()
    };
    assert_eq!(
        ids(&bare),
        ids(&canonical),
        "both spellings resume inclusively of the stamp tie"
    );
    assert_eq!(ids(&canonical), vec!["i-a".to_owned(), "i-b".to_owned()]);
    // The compound pair still resumes strictly after its entity part.
    assert_eq!(ids(&format!("{stamp}\u{1f}i-a")), vec!["i-b".to_owned()]);
}

fn feed_item(id: &str, bank: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: "subj".into(),
        kind: ItemKind::Episodic,
        episodic_type: Some(clio_types::EpisodicType::Triple),
        category: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        admission_score: Some(0.9),
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

fn am_store_assoc(a: &str, b: &str) -> crate::model::AssocEdgeRecord {
    crate::model::AssocEdgeRecord {
        id: crate::assoc::coactivation_id("b1", a, b),
        bank_id: "b1".into(),
        source_id: a.into(),
        target_id: b.into(),
        assoc_kind: crate::model::AssocKind::Coactivation,
        relationship: None,
        weight: 0.25,
        last_updated_at: "2026-01-02T00:00:00Z".into(),
    }
}

/// A fresh push cursor uses the empty-watermark sentinel
/// `1970-01-01T00:00:00Z`; rows whose change stamp is epoch-millis text
/// (e.g. a task/failure carrier item) sort below that sentinel and were
/// skipped. The sentinel must include every bank row.
#[test]
fn feed_items_sentinel_includes_epoch_millis_stamp() {
    let s = store();
    let mut item = feed_item("millis-1", "b1");
    let stamp = "1735689600000";
    item.valid_from = stamp.into();
    item.tx_from = stamp.into();
    item.created_at = stamp.into();
    item.updated_at = stamp.into();
    s.create_memory_item(&item, "actor").expect("create");
    let feed = s
        .feed_items("b1", "1970-01-01T00:00:00Z", 10)
        .expect("feed");
    assert!(
        feed.iter().any(|r| r.entity_id == "millis-1"),
        "epoch-millis rows must not fall below the empty-watermark sentinel: {feed:?}"
    );
}

#[test]
fn feed_limit_caps_rows() {
    let s = store();
    for n in 0..3 {
        let item = feed_item(&format!("lim-{n}"), "b1");
        s.create_memory_item(&item, "actor").expect("create");
    }
    assert_eq!(
        s.feed_items("b1", "2025-12-31T00:00:00Z", 2)
            .expect("feed")
            .len(),
        2
    );
}

#[test]
fn belief_context_correction_is_visible_to_the_change_feed() {
    // A correction must reach peers, so it bumps the belief's change
    // stamp. Without that, a peer keeps the old context forever.
    use crate::BeliefStore as _;
    let s = store();
    let bank = "b-correct";
    let mut seeded = crate::belief_store::belief_store_belief_tests::mk_belief(
        "bel-correct",
        bank,
        "subj-correct",
    );
    seeded.context = Some("original".into());
    s.create_belief(&seeded, "agent").expect("create");
    // After the seed's own stamp: nothing else is pending yet.
    let watermark = "2026-06-02T00:00:00Z";
    assert!(
        s.feed_beliefs(bank, watermark, 10)
            .expect("feed")
            .is_empty(),
        "the seed must be behind the watermark"
    );

    s.correct_belief_context(bank, &seeded.id, "corrected", "test correction", "agent")
        .expect("correct");

    let feed = s.feed_beliefs(bank, watermark, 10).expect("feed");
    assert_eq!(
        feed.len(),
        1,
        "the correction must be fed to peers: {feed:?}"
    );
    assert_eq!(feed[0].1.context.as_deref(), Some("corrected"));
}
