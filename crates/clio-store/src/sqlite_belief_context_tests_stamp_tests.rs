// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the SQLite belief change-stamp and effective-
//! time regression tests: the monotonic `updated_at` change stamp across older
//! appends and the store-derived `context_as_of` stamp lifecycle.
//!
//! ## Owns
//! - `sqlite_belief_change_stamp_never_regresses_past_older_appends`
//! - `sqlite_belief_context_as_of_is_store_derived`
//!
//! ## Does not own
//! - The stamp SQL itself (`belief_tx`, `belief_context_correct`)
//! - Correction value/audit assertions (`belief_context_sqlite_tests`)
//!
//! ## Boundary
//!
//! Changes to stamp or effective-time behavior should update tests here.
//! Changes to correction value/audit assertions should not require changes here.
//!
//! Test-only. Never compiled into production builds.

use crate::BeliefStore as _;
use crate::SyncStore as _;
use crate::belief_store::belief_store_belief_tests::mk_belief;
use crate::dek::{DekProvider, LocalDevKms};
use crate::migrate::IntoAmError;
use crate::sqlite::SqliteStore;
use crate::test_unique::unique;
use clio_types::{ConfidenceEntry, SourceType};
use rusqlite::params;
use std::sync::Arc;

fn open_store() -> SqliteStore {
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    SqliteStore::open_in_memory(kms).unwrap()
}

fn read_beliefs_row(store: &SqliteStore, bank: &str, id: &str) -> (String, Option<String>) {
    store
        .with_conn(|conn| {
            conn.query_row(
                "SELECT updated_at, context_as_of FROM beliefs
                 WHERE id = ?1 AND bank_id = ?2",
                params![id, bank],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .am("beliefs stamp row")
        })
        .unwrap()
}

/// The belief change stamp must be monotonic. An append whose entry
/// `as_of` is older than the correction stamp must not move `updated_at`
/// backwards, and the change feed (`COALESCE(updated_at, created_at) >
/// watermark`) must still select the row afterwards.
#[test]
fn sqlite_belief_change_stamp_never_regresses_past_older_appends() {
    let store = open_store();
    let bank = unique("bank-bmonotonic");
    let id = unique("bel-bmonotonic");
    let mut b = mk_belief(&id, &bank, &unique("subj-bmonotonic"));
    b.context = Some("original context".into());
    store.create_belief(&b, "agent").unwrap();
    let stamp_after_create = read_beliefs_row(&store, &bank, &id).0;

    store
        .correct_belief_context(&bank, &id, "corrected context", "operator fix", "agent")
        .unwrap();
    let stamp_after_correct = read_beliefs_row(&store, &bank, &id).0;
    assert!(
        stamp_after_correct.as_str() > stamp_after_create.as_str(),
        "the correction must move the stamp forward"
    );

    // A late-arriving observation whose `as_of` predates the correction stamp.
    let late =
        ConfidenceEntry::new("2026-01-01", 0.55, "turn_late", SourceType::AgentInferred).unwrap();
    store.append_confidence(&bank, &id, &late, "agent").unwrap();
    assert_eq!(
        read_beliefs_row(&store, &bank, &id).0,
        stamp_after_correct,
        "an older `as_of` must not regress beliefs.updated_at"
    );

    // The change feed still selects the row and carries the corrected context.
    let feed = store
        .feed_beliefs(&bank, "2026-06-02T00:00:00Z", 10)
        .unwrap();
    assert_eq!(feed.len(), 1, "the row must stay selected: {feed:?}");
    assert_eq!(feed[0].0, stamp_after_correct);
    assert_eq!(feed[0].1.context.as_deref(), Some("corrected context"));
}

/// `context_as_of` is store-derived — set from `created_at` at create,
/// moved to the correction stamp by an authorized correction, left NULL
/// without a context, and never taken from a caller (sync apply cannot forge).
#[test]
fn sqlite_belief_context_as_of_is_store_derived() {
    let store = open_store();
    let bank = unique("bank-bstamp");
    let id = unique("bel-bstamp");
    let mut b = mk_belief(&id, &bank, &unique("subj-bstamp"));
    b.context = Some("original context".into());
    store.create_belief(&b, "agent").unwrap();
    let got = store.get_belief(&bank, &id).unwrap().expect("belief");
    assert_eq!(
        got.context_as_of.as_deref(),
        Some(b.created_at.as_str()),
        "create-with-context is effective from created_at"
    );

    store
        .correct_belief_context(&bank, &id, "corrected context", "operator fix", "agent")
        .unwrap();
    let (updated_at, context_as_of) = read_beliefs_row(&store, &bank, &id);
    let stamp = context_as_of.expect("correction stamps the effective time");
    assert!(stamp.as_str() >= b.created_at.as_str());
    // The effective-time stamp equals the row's monotonic change stamp.
    assert_eq!(updated_at, stamp);

    // A caller-supplied stamp is ignored: the store derives it.
    let forged = unique("bel-bstamp-forged");
    let mut f = mk_belief(&forged, &bank, &unique("subj-bstamp"));
    f.proposition = "User Prefers Rooibos Tea".into();
    f.context = Some("forged context".into());
    f.context_as_of = Some("2020-01-01".into());
    store.create_belief(&f, "agent").unwrap();
    let got = store.get_belief(&bank, &forged).unwrap().expect("belief");
    assert_eq!(got.context_as_of.as_deref(), Some(f.created_at.as_str()));

    // Create without a context leaves the stamp NULL.
    let bare = unique("bel-bstamp-bare");
    let mut none = mk_belief(&bare, &bank, &unique("subj-bstamp"));
    none.proposition = "User Prefers Chamomile Tea".into();
    none.context = None;
    store.create_belief(&none, "agent").unwrap();
    let got = store.get_belief(&bank, &bare).unwrap().expect("belief");
    assert_eq!(got.context, None);
    assert_eq!(got.context_as_of, None);
}
