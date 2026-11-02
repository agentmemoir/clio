// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the SQLite persona write transaction bodies.
//!
//! ## Owns
//! - `put_stable_entry_tx_audited`, `put_preference_state_tx_audited`, and the
//!   prior-context readers
//!
//! ## Does not own
//! - The persona read path or the Postgres twin
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::migrate::IntoAmError;
use crate::persona_store::PersonaStore;
use crate::persona_store::persona_store_persona_tests::{pref, stable};
use crate::sqlite::sqlite_history_tests::store;
use crate::test_unique::unique;
use clio_types::ErrorCode;

fn audit_detail(
    store: &crate::sqlite::SqliteStore,
    bank: &str,
    item_id: &str,
) -> serde_json::Value {
    let raw: String = store
        .with_conn(|conn| {
            conn.query_row(
                "SELECT detail_json FROM audit_events \
                 WHERE bank_id = ?1 AND item_id = ?2 \
                 ORDER BY created_at DESC, id DESC LIMIT 1",
                rusqlite::params![bank, item_id],
                |r| r.get(0),
            )
            .am("test read")
        })
        .unwrap();
    serde_json::from_str(&raw).unwrap()
}

#[test]
#[allow(clippy::too_many_lines)]
fn stable_write_preserves_context_and_audits_stats() {
    let store = store();
    let bank = unique("sqlite-persona-stable");
    let prior = "prior stable note";

    let mut first = stable(&bank, "style", "terse", 0.9, "2026-09-10");
    first.context = Some(prior.into());
    store.put_stable_entry(&first, "agent").unwrap();
    let mut second = stable(&bank, "style", "detailed", 0.7, "2026-09-11");
    second.context = None;
    store.put_stable_entry(&second, "agent").unwrap();

    let detail = audit_detail(&store, &bank, "persona-stable:style");
    assert_eq!(detail["context_action"], "preserved");
    assert_eq!(detail["context_prior_present"], true);
    assert_eq!(detail["context_prior_len"], prior.len());
    assert_eq!(detail["context_present"], true);
    let rendered = detail.to_string();
    assert!(!rendered.contains(prior), "{rendered}");
    assert!(!rendered.contains("cipher"), "{rendered}");

    let history = store.stable_history(&bank, "style").unwrap();
    assert_eq!(history.len(), 2);
    assert!(
        history
            .iter()
            .all(|entry| entry.context.as_deref() == Some(prior))
    );
}

#[test]
fn preference_write_preserves_context_and_audits_stats() {
    let store = store();
    let bank = unique("sqlite-persona-pref");
    let prior = "week one note";

    let mut first = pref(&bank, "verbosity", 0.4, 0.1, 0.6);
    first.context = Some(prior.into());
    store.put_preference_state(&first, "agent").unwrap();
    let mut second = pref(&bank, "verbosity", 0.5, 0.2, 0.6);
    second.context = None;
    store.put_preference_state(&second, "agent").unwrap();

    let detail = audit_detail(&store, &bank, "persona-preference:verbosity");
    assert_eq!(detail["context_action"], "preserved");
    assert_eq!(detail["context_prior_present"], true);
    assert_eq!(detail["context_prior_len"], prior.len());

    let stored = store
        .get_preference(&bank, "verbosity")
        .unwrap()
        .expect("preference");
    assert_eq!(stored.context.as_deref(), Some(prior));
}

#[test]
fn erased_bank_refuses_a_persona_write() {
    let store = store();
    let bank = unique("sqlite-persona-erased");
    store
        .test_execute(&format!(
            "INSERT INTO persona_erase_tombstones (bank_id, erased_at) \
             VALUES ('{bank}', '2026-01-01T00:00:00Z')"
        ))
        .unwrap();
    let err = store
        .put_stable_entry(&stable(&bank, "style", "terse", 0.9, "2026-09-10"), "agent")
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::ErasedSubject);
    assert_eq!(err.message, "erased persona context resurrection");
    let pref_err = store
        .put_preference_state(&pref(&bank, "verbosity", 0.4, 0.1, 0.6), "agent")
        .unwrap_err();
    assert_eq!(pref_err.code, ErrorCode::ErasedSubject);
}
