// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the Postgres persona write transaction bodies.
//!
//! ## Owns
//! - `put_stable_entry_tx_audited`, `put_preference_state_tx_audited`, and the
//!   prior-context readers
//!
//! ## Does not own
//! - The persona read path or the SQLite twin
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::migrate::IntoAmError;
use crate::persona_store::PersonaStore;
use crate::persona_store::persona_store_persona_tests::{pref, stable};
use crate::postgres_belief::postgres_belief_context_tests::open_pg;
use crate::test_unique::unique;
use clio_types::ErrorCode;

fn audit_detail(
    store: &crate::postgres::PostgresStore,
    bank: &str,
    item_id: &str,
) -> serde_json::Value {
    let raw: String = store
        .with_client(|client| {
            client
                .query_one(
                    "SELECT detail_json FROM audit_events \
                     WHERE bank_id = $1 AND item_id = $2 \
                     ORDER BY created_at DESC, id DESC LIMIT 1",
                    &[&bank, &item_id],
                )
                .am("test read")
        })
        .unwrap()
        .get(0);
    serde_json::from_str(&raw).unwrap()
}

#[test]
#[allow(clippy::too_many_lines)]
fn postgres_persona_writes_preserve_context_and_audit_stats() {
    let (_guard, store, _kms) = open_pg();
    let bank = unique("pg-persona-write");
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

    let preference_note = "week one note";
    let mut preference = pref(&bank, "verbosity", 0.4, 0.1, 0.6);
    preference.context = Some(preference_note.into());
    store.put_preference_state(&preference, "agent").unwrap();
    let mut updated = pref(&bank, "verbosity", 0.5, 0.2, 0.6);
    updated.context = None;
    store.put_preference_state(&updated, "agent").unwrap();
    let pref_detail = audit_detail(&store, &bank, "persona-preference:verbosity");
    assert_eq!(pref_detail["context_action"], "preserved");
    assert_eq!(pref_detail["context_prior_present"], true);
    assert_eq!(pref_detail["context_prior_len"], preference_note.len());
    let stored = store
        .get_preference(&bank, "verbosity")
        .unwrap()
        .expect("preference");
    assert_eq!(stored.context.as_deref(), Some(preference_note));
}

#[test]
fn postgres_erased_bank_refuses_a_persona_write() {
    let (_guard, store, _kms) = open_pg();
    let bank = unique("pg-persona-erased");
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
    let pref_err = store
        .put_preference_state(&pref(&bank, "verbosity", 0.4, 0.1, 0.6), "agent")
        .unwrap_err();
    assert_eq!(pref_err.code, ErrorCode::ErasedSubject);
}
