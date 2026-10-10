// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the SQLite fixtures that pin the retained
//! preference-context effective-time fallback in `preference_stored_context`.
//!
//! ## Owns
//! - The fallback fixture: a row with a stored context and NULL `context_as_of`
//!   (the pre-column shape) reads with the row `as_of`; a stored
//!   `context_as_of` keeps its value
//!
//! ## Does not own
//! - Point-level context gating in continuous-series reads (`attribute_tests`)
//! - Postgres parity for continuous attributes (`pg_attribute_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the retained preference-context fallback tests.

use crate::dek::{DekProvider, LocalDevKms};
use crate::persona_store::PersonaStore;
use crate::sqlite::SqliteStore;
use crate::test_unique::unique;
use clio_types::PersonaPreference;
use std::sync::Arc;

fn store() -> SqliteStore {
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    SqliteStore::open_in_memory(kms).expect("sqlite")
}

fn preference(bank: &str, key: &str, context: Option<&str>, as_of: &str) -> PersonaPreference {
    PersonaPreference {
        bank_id: bank.into(),
        key: key.into(),
        value: 0.8,
        trend: 0.8,
        confidence: None,
        source: "explicit".into(),
        source_ref: None,
        context: context.map(str::to_owned),
        admission_score: 0.9,
        as_of: as_of.into(),
    }
}

/// The retained fallback: a NULL `context_as_of` (the shape the SQLite
/// context-column convergence leaves on pre-existing preference rows)
/// substitutes the row `as_of`; a stored `context_as_of` keeps its value.
#[test]
fn preference_context_effective_time_falls_back_to_row_as_of_when_null() {
    let s = store();
    let bank = unique("b");

    // Pre-column shape: stored context cipher with NULL context_as_of. The row
    // is written through the current writer, which stamps context_as_of, then
    // set back to the converged shape.
    let pre_column = preference(&bank, "tone", Some("pre-column context"), "2026-01-15");
    s.put_preference_state(&pre_column, "seeder").unwrap();
    s.test_execute(&format!(
        "UPDATE persona_preferences SET context_as_of = NULL
         WHERE bank_id = '{bank}' AND attr_key = 'tone'"
    ))
    .unwrap();

    let (text, effective) = s
        .preference_stored_context(&bank, "tone")
        .unwrap()
        .expect("stored context");
    assert_eq!(text, "pre-column context");
    assert_eq!(
        effective, "2026-01-15",
        "NULL context_as_of must fall back to the row as_of"
    );

    // Current shape: the context became effective at the first write's as_of;
    // a later ordinary update without a context preserves the stored time.
    let mut current = preference(&bank, "pace", Some("stored context time"), "2026-02-01");
    s.put_preference_state(&current, "seeder").unwrap();
    current.as_of = "2026-03-01".into();
    current.context = None;
    s.put_preference_state(&current, "seeder").unwrap();

    let (text, effective) = s
        .preference_stored_context(&bank, "pace")
        .unwrap()
        .expect("stored context");
    assert_eq!(text, "stored context time");
    assert_eq!(
        effective, "2026-02-01",
        "a stored context_as_of must win over the row as_of"
    );
}
