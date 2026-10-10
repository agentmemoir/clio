// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the Postgres persona/belief record apply primitives on
//! the shared Compose `clio` database: the `SyncStore` write/read delegation
//! methods behind the apply engine (stable entries, preferences, beliefs)
//! and the row-mapping body of `feed_beliefs` with real belief rows.
//!
//! ## Owns
//! - Postgres parity coverage for the record apply `SyncStore` primitives
//!
//! ## Does not own
//! - Journal / cursor / DLQ parity tests (`postgres_sync` `pg_sync_tests`)
//! - Apply semantics (`clio-sync`)
//!
//! ## Boundary
//!
//! Tests here break when Postgres record SQL changes; they never change it.
//!
//! Keep this module focused on Postgres record apply primitives.

use crate::db_url::postgres_url_from_env;
use crate::pg_lock::lock_pg_test;
use crate::sync_store::SyncStore;
use crate::{BeliefStore, DekProvider, LocalDevKms, PostgresStore};
use std::sync::Arc;

fn open_pg() -> PostgresStore {
    let url = postgres_url_from_env();
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    PostgresStore::open(&url, kms).expect("postgres")
}

use crate::test_unique::unique;

/// The apply side writes persona stable entries through the `SyncStore`
/// primitive and reads the current open version back (server-blind value).
#[test]
fn pg_sync_stable_put_and_current_roundtrip() {
    let _guard = lock_pg_test();
    let bank = unique("bank");
    let s = open_pg();
    assert!(
        s.persona_stable_current(&bank, "style")
            .expect("current")
            .is_none()
    );
    let entry = clio_types::PersonaStableEntry {
        bank_id: bank.clone(),
        key: "style".into(),
        value: "terse".into(),
        confidence: Some(0.8),
        source: "explicit".into(),
        source_ref: Some("ev".into()),
        context: Some("why recorded".into()),
        admission_score: 0.5,
        as_of: "2026-01-01".into(),
        valid_until: None,
    };
    SyncStore::persona_stable_put(&s, &entry, "sync-apply").expect("put");
    let cur = s
        .persona_stable_current(&bank, "style")
        .expect("current")
        .expect("row");
    assert_eq!(cur.value, "terse");
    assert_eq!(cur.context.as_deref(), Some("why recorded"));
    assert_eq!(cur.confidence, Some(0.8));
    assert_eq!(cur.valid_until, None);
}

/// The preference apply primitives round-trip through `persona_preferences`
/// and surface the current row only after a put.
#[test]
fn pg_sync_preference_put_and_current_roundtrip() {
    let _guard = lock_pg_test();
    let bank = unique("bank");
    let s = open_pg();
    assert!(
        s.persona_preference_current(&bank, "brevity")
            .expect("current")
            .is_none()
    );
    let pref = clio_types::PersonaPreference {
        bank_id: bank.clone(),
        key: "brevity".into(),
        value: 0.4,
        trend: 0.1,
        confidence: None,
        source: "inferred".into(),
        source_ref: None,
        context: Some("preference context".into()),
        admission_score: 0.5,
        as_of: "2026-01-01".into(),
    };
    SyncStore::persona_preference_put(&s, &pref, "sync-apply").expect("put");
    let cur = s
        .persona_preference_current(&bank, "brevity")
        .expect("current")
        .expect("row");
    // `value` is an `f64` column that round-trips bit-exactly through the
    // store; compare with an explicit tolerance instead of clippy's strict
    // float equality (a discrete key is not available for this column).
    assert!((cur.value - 0.4).abs() < 1e-12, "value = {}", cur.value);
    assert_eq!(cur.context.as_deref(), Some("preference context"));
    assert_eq!(cur.as_of, "2026-01-01");
}

/// Belief create / get / by-proposition / append under the `SyncStore`
/// contract: the append-only trajectory grows and the proposition lookup
/// finds the durable row (and `None` for unknown propositions).
#[test]
fn pg_sync_belief_create_get_proposition_append_roundtrip() {
    let _guard = lock_pg_test();
    let bank = unique("bank");
    let s = open_pg();
    let belief = clio_types::BeliefObject {
        id: format!("bel-{}", unique("pg")),
        bank_id: bank.clone(),
        subject_id: "s1".into(),
        proposition: "user prefers oat milk".into(),
        context: Some("stated twice".into()),
        context_as_of: None,
        confidence_history: vec![
            clio_types::ConfidenceEntry::new(
                "2026-01-01",
                0.7,
                "ev",
                clio_types::SourceType::UserStated,
            )
            .expect("entry"),
        ],
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-01-01T00:00:00Z".into(),
    };
    SyncStore::belief_create(&s, &belief, "sync-apply").expect("create");
    assert!(
        s.belief_by_proposition(&bank, "unknown proposition")
            .expect("by proposition")
            .is_none()
    );
    let found = s
        .belief_by_proposition(&bank, &clio_types::proposition_key(&belief.proposition))
        .expect("by proposition")
        .expect("row");
    assert_eq!(found.id, belief.id);
    assert_eq!(found.proposition, belief.proposition);
    assert_eq!(found.context.as_deref(), Some("stated twice"));
    let later = clio_types::ConfidenceEntry::new(
        "2026-01-02",
        0.9,
        "ev-2",
        clio_types::SourceType::UserStated,
    )
    .expect("entry");
    s.belief_append(&bank, &belief.id, &later, "sync-apply", None)
        .expect("append");
    let got = s.belief_get(&bank, &belief.id).expect("get").expect("row");
    assert_eq!(got.confidence_history.len(), 2);
    // `confidence` is an `f64` column that round-trips bit-exactly; compare
    // with an explicit tolerance instead of strict float equality.
    assert!(
        (got.confidence_history[1].confidence - 0.9).abs() < 1e-12,
        "confidence = {}",
        got.confidence_history[1].confidence
    );
    assert!(s.belief_get(&bank, "bel-none").expect("get").is_none());
}

/// `feed_beliefs` maps returned rows: opened proposition and context text,
/// the per-belief trajectory joined back, and the change stamp.
#[test]
fn pg_sync_feed_beliefs_maps_rows_with_history_and_stamp() {
    let _guard = lock_pg_test();
    let bank = unique("bank");
    let s = open_pg();
    let initial_feed = s
        .feed_beliefs(&bank, "1970-01-01T00:00:00Z", 200)
        .expect("feed");
    assert!(initial_feed.is_empty(), "{initial_feed:?}");
    let mut belief = clio_types::BeliefObject {
        id: format!("bel-{}", unique("pg")),
        bank_id: bank.clone(),
        subject_id: "s1".into(),
        proposition: "user prefers terse replies".into(),
        context: Some("kept".into()),
        context_as_of: None,
        confidence_history: vec![
            clio_types::ConfidenceEntry::new(
                "2026-01-01",
                0.6,
                "ev-1",
                clio_types::SourceType::UserStated,
            )
            .expect("entry"),
        ],
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-01-01T00:00:00Z".into(),
    };
    s.create_belief(&belief, "seed").expect("create");
    let later = clio_types::ConfidenceEntry::new(
        "2026-01-02",
        0.9,
        "ev-2",
        clio_types::SourceType::AgentInferred,
    )
    .expect("entry");
    s.belief_append(&bank, &belief.id, &later, "seed", None)
        .expect("append");
    belief.confidence_history.push(later);
    let rows = s
        .feed_beliefs(&bank, "1970-01-01T00:00:00Z", 200)
        .expect("feed");
    assert_eq!(rows.len(), 1);
    let (stamp, got) = &rows[0];
    assert_eq!(got.id, belief.id);
    assert_eq!(got.bank_id, bank);
    assert_eq!(got.proposition, belief.proposition);
    assert_eq!(got.context.as_deref(), Some("kept"));
    assert_eq!(got.confidence_history.len(), 2);
    assert_eq!(got.confidence_history[1].as_of, "2026-01-02");
    assert_eq!(
        got.confidence_history[1].source_type,
        clio_types::SourceType::AgentInferred
    );
    assert_eq!(got.updated_at, "2026-01-02");
    assert!(!stamp.is_empty(), "each row carries its change stamp");
    // A later watermark filters the row out.
    let later_feed = s
        .feed_beliefs(&bank, "9999-01-01T00:00:00Z", 200)
        .expect("feed");
    assert!(later_feed.is_empty(), "{later_feed:?}");
}

/// Parity: a belief context correction on Postgres bumps the change stamp,
/// so the corrected context replicates to peers instead of staying local.
#[test]
fn pg_belief_context_correction_is_visible_to_the_change_feed() {
    let _guard = lock_pg_test();
    let bank = unique("bank");
    let s = open_pg();
    let belief = clio_types::BeliefObject {
        id: format!("bel-{}", unique("pg")),
        bank_id: bank.clone(),
        subject_id: "s1".into(),
        proposition: "user prefers rye bread".into(),
        context: Some("original".into()),
        context_as_of: None,
        confidence_history: vec![
            clio_types::ConfidenceEntry::new(
                "2026-01-01",
                0.7,
                "ev",
                clio_types::SourceType::UserStated,
            )
            .expect("entry"),
        ],
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-01-01T00:00:00Z".into(),
    };
    s.create_belief(&belief, "agent").expect("create");
    let watermark = "2026-06-02T00:00:00Z";
    assert!(
        s.feed_beliefs(&bank, watermark, 10)
            .expect("feed")
            .is_empty(),
        "the seed must be behind the watermark"
    );
    s.correct_belief_context(&bank, &belief.id, "corrected", "test correction", "agent")
        .expect("correct");
    let feed = s.feed_beliefs(&bank, watermark, 10).expect("feed");
    assert_eq!(
        feed.len(),
        1,
        "the correction must be fed to peers: {feed:?}"
    );
    assert_eq!(feed[0].1.context.as_deref(), Some("corrected"));
}
