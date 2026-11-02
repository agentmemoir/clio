// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the SQLite persona/belief sync change feeds.
//!
//! ## Owns
//! - `feed_persona_stable_sqlite`, `feed_persona_prefs_sqlite`,
//!   `feed_beliefs_sqlite`, and the bounded trajectory load
//!
//! ## Does not own
//! - Item/assoc/triple feeds (`sync_records`) or the persona write path
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

#![allow(clippy::assert_is_empty)]

use crate::belief_store::BeliefStore;
use crate::belief_store::belief_store_belief_tests::mk_belief;
use crate::migrate::IntoAmError;
use crate::persona_store::PersonaStore;
use crate::persona_store::persona_store_persona_tests::{pref, stable};
use crate::sqlite::sqlite_history_tests::store;
use crate::test_unique::unique;
use clio_types::{ConfidenceEntry, ErrorCode, SourceType};

const EPOCH: &str = "1970-01-01T00:00:00Z";
const LATER: &str = "9999-01-01T00:00:00Z";

fn entry(as_of: &str) -> ConfidenceEntry {
    ConfidenceEntry::new(as_of, 0.7, "turn_9", SourceType::AgentInferred).expect("entry")
}

#[test]
#[allow(clippy::too_many_lines)]
fn persona_and_belief_feeds_open_their_rows() {
    let store = store();
    let bank = unique("sync-persona");
    let mut stable_entry = stable(&bank, "style", "terse", 0.9, "2026-06-01");
    stable_entry.context = Some("stable ctx".into());
    store.put_stable_entry(&stable_entry, "agent").unwrap();

    let stable_feed = super::feed_persona_stable_sqlite(&store, &bank, EPOCH, 10).unwrap();
    assert_eq!(stable_feed.len(), 1);
    assert!(!stable_feed[0].0.is_empty());
    assert_eq!(stable_feed[0].1.value, "terse");
    assert_eq!(stable_feed[0].1.context.as_deref(), Some("stable ctx"));
    assert!(
        super::feed_persona_stable_sqlite(&store, &bank, LATER, 10)
            .unwrap()
            .is_empty()
    );

    let mut preference = pref(&bank, "verbosity", 0.4, 0.1, 0.6);
    preference.context = Some("pref ctx".into());
    store.put_preference_state(&preference, "agent").unwrap();
    let pref_feed = super::feed_persona_prefs_sqlite(&store, &bank, EPOCH, 10).unwrap();
    assert_eq!(pref_feed.len(), 1);
    assert!((pref_feed[0].1.value - 0.4).abs() < 1e-12);
    assert_eq!(pref_feed[0].1.context.as_deref(), Some("pref ctx"));

    let belief_id = unique("sync-belief");
    let belief_subject = unique("sync-belief-subject");
    let mut belief = mk_belief(&belief_id, &bank, &belief_subject);
    belief.context = Some("belief ctx".into());
    store.create_belief(&belief, "agent").unwrap();
    store
        .append_confidence(&bank, &belief_id, &entry("2026-06-02"), "agent")
        .unwrap();

    let belief_feed = super::feed_beliefs_sqlite(&store, &bank, EPOCH, 10).unwrap();
    assert_eq!(belief_feed.len(), 1);
    assert_eq!(belief_feed[0].1.proposition, belief.proposition);
    assert_eq!(belief_feed[0].1.context.as_deref(), Some("belief ctx"));
    assert_eq!(belief_feed[0].1.confidence_history.len(), 2);
    assert_eq!(belief_feed[0].1.updated_at, "2026-06-02");

    let empty = unique("sync-persona-empty");
    assert!(
        super::feed_persona_stable_sqlite(&store, &empty, EPOCH, 10)
            .unwrap()
            .is_empty()
    );
    assert!(
        super::feed_persona_prefs_sqlite(&store, &empty, EPOCH, 10)
            .unwrap()
            .is_empty()
    );
    assert!(
        super::feed_beliefs_sqlite(&store, &empty, EPOCH, 10)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn an_older_version_cipher_does_not_open_in_the_open_row() {
    let store = store();
    let bank = unique("sync-feed-version-binding");
    // Same as_of on purpose: the stored row id, not the version time, keeps
    // the two ciphertext identities apart.
    store
        .put_stable_entry(&stable(&bank, "tone", "warm", 0.9, "2026-06-01"), "agent")
        .unwrap();
    store
        .put_stable_entry(&stable(&bank, "tone", "cool", 0.7, "2026-06-01"), "agent")
        .unwrap();
    let closed: String = store
        .with_conn(|conn| {
            conn.query_row(
                "SELECT value_cipher FROM persona_stable_entries \
                 WHERE bank_id = ?1 AND attr_key = 'tone' AND valid_until IS NOT NULL",
                rusqlite::params![bank],
                |r| r.get(0),
            )
            .am("test read")
        })
        .unwrap();
    store
        .test_execute(&format!(
            "UPDATE persona_stable_entries SET value_cipher = '{closed}' \
             WHERE bank_id = '{bank}' AND attr_key = 'tone' AND valid_until IS NULL"
        ))
        .unwrap();
    assert_eq!(
        store.stable_history(&bank, "tone").unwrap_err().code,
        ErrorCode::Forbidden
    );
}

#[test]
fn feeds_fail_closed_on_a_swapped_cipher() {
    let store = store();
    let bank = unique("sync-feed-binding");
    let first = stable(&bank, "style", "terse", 0.9, "2026-06-01");
    let second = stable(&bank, "plain", "wordy", 0.8, "2026-06-01");
    store.put_stable_entry(&first, "agent").unwrap();
    store.put_stable_entry(&second, "agent").unwrap();
    let cipher: String = store
        .with_conn(|conn| {
            conn.query_row(
                "SELECT value_cipher FROM persona_stable_entries \
                 WHERE bank_id = ?1 AND attr_key = 'style' AND valid_until IS NULL",
                rusqlite::params![bank],
                |r| r.get(0),
            )
            .am("test read")
        })
        .unwrap();
    store
        .test_execute(&format!(
            "UPDATE persona_stable_entries SET value_cipher = '{cipher}' \
             WHERE bank_id = '{bank}' AND attr_key = 'plain'"
        ))
        .unwrap();
    assert_eq!(
        super::feed_persona_stable_sqlite(&store, &bank, EPOCH, 10)
            .unwrap_err()
            .code,
        ErrorCode::Forbidden
    );

    let belief_bank = unique("sync-feed-belief-binding");
    let first_id = unique("sync-feed-belief-a");
    let second_id = unique("sync-feed-belief-b");
    store
        .create_belief(
            &mk_belief(&first_id, &belief_bank, &unique("sync-feed-subject-a")),
            "agent",
        )
        .unwrap();
    let mut second_belief = mk_belief(&second_id, &belief_bank, &unique("sync-feed-subject-b"));
    second_belief.proposition = unique("sync-feed-proposition-b");
    store.create_belief(&second_belief, "agent").unwrap();
    let proposition: String = store
        .with_conn(|conn| {
            conn.query_row(
                "SELECT proposition_ciphertext FROM beliefs WHERE id = ?1",
                rusqlite::params![first_id],
                |r| r.get(0),
            )
            .am("test read")
        })
        .unwrap();
    store
        .test_execute(&format!(
            "UPDATE beliefs SET proposition_ciphertext = '{proposition}' WHERE id = '{second_id}'"
        ))
        .unwrap();
    assert_eq!(
        super::feed_beliefs_sqlite(&store, &belief_bank, EPOCH, 10)
            .unwrap_err()
            .code,
        ErrorCode::Forbidden
    );
}
