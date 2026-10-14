// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for persona-`context` coverage of the bundle
//! import path: whether a differing stable or preference `context` is applied,
//! skipped, or deduplicated under force and without it.
//!
//! ## Owns
//! - Persona-context import assertions over the shared `import_fixture` harness
//!
//! ## Does not own
//! - Fixture builders (see `import_fixture.rs`)
//! - Item-context import assertions (see `import_context_tests.rs`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::bundle::{ExportContentMode, ExportEntities};
use crate::import::import;
use crate::import_fixture::{Harness, write_bundle};
use clio_store::PersonaStore;
use clio_types::{PersonaPreference, PersonaStableEntry};

const TS: &str = "2026-06-01T00:00:00Z";

/// A seeded persona stable row (bank-local) and the matching bundle row with a
/// different `context` but identical `key`/`value`/`as_of`.
fn stable_pair(bank: &str) -> (PersonaStableEntry, PersonaStableEntry) {
    let seeded = PersonaStableEntry {
        context: Some("old why".into()),
        bank_id: bank.to_owned(),
        key: "communication_style".into(),
        value: "concise".into(),
        confidence: Some(0.8),
        source: "explicit".into(),
        source_ref: Some("turn_1".into()),
        admission_score: 0.8,
        as_of: TS.into(),
        valid_until: None,
    };
    let mut bundled = seeded.clone();
    bundled.bank_id = "exported-bank".into();
    bundled.context = Some("new why".into());
    (seeded, bundled)
}

/// A seeded persona preference row and the matching bundle row with a
/// different `context` but identical `value`/`trend`/`as_of`.
fn preference_pair(bank: &str) -> (PersonaPreference, PersonaPreference) {
    let seeded = PersonaPreference {
        context: Some("old pref why".into()),
        bank_id: bank.to_owned(),
        key: "brevity_affinity".into(),
        value: 0.7,
        trend: 0.6,
        confidence: None,
        source: "explicit".into(),
        source_ref: None,
        admission_score: 0.7,
        as_of: TS.into(),
    };
    let mut bundled = seeded.clone();
    bundled.bank_id = "exported-bank".into();
    bundled.context = Some("new pref why".into());
    (seeded, bundled)
}

#[test]
fn differing_stable_context_import_applies_under_force() {
    let h = Harness::new();
    let (seeded, bundled) = stable_pair(&h.bank);
    h.store.put_stable_entry(&seeded, "tester").unwrap();
    let bundle = write_bundle(
        &ExportEntities {
            persona_stable: vec![bundled],
            ..ExportEntities::default()
        },
        ExportContentMode::DsarPlaintext,
    );

    let report = import(&h.args(
        &h.ctx(),
        h.policy(),
        bundle.path(),
        false,
        true,
        Some("restore context"),
    ))
    .expect("force import succeeds");
    assert_eq!(
        report.would_overwrite, 1,
        "differing context must overwrite"
    );

    let history = h
        .store
        .stable_history(&h.bank, "communication_style")
        .unwrap();
    assert!(
        history
            .iter()
            .any(|e| e.context.as_deref() == Some("new why")),
        "the bundled context must be preserved through import"
    );
}

#[test]
fn differing_preference_context_import_applies_under_force() {
    let h = Harness::new();
    let (seeded, bundled) = preference_pair(&h.bank);
    h.store.put_preference_state(&seeded, "tester").unwrap();
    let bundle = write_bundle(
        &ExportEntities {
            persona_preferences: vec![bundled],
            ..ExportEntities::default()
        },
        ExportContentMode::DsarPlaintext,
    );

    let report = import(&h.args(
        &h.ctx(),
        h.policy(),
        bundle.path(),
        false,
        true,
        Some("restore context"),
    ))
    .expect("force import succeeds");
    assert_eq!(
        report.would_overwrite, 1,
        "differing context must overwrite"
    );

    let stored = h
        .store
        .get_preference(&h.bank, "brevity_affinity")
        .unwrap()
        .expect("preference exists");
    assert_eq!(stored.context.as_deref(), Some("new pref why"));
}

#[test]
fn differing_context_import_without_force_preserves_stored_context() {
    let h = Harness::new();
    let (seeded, bundled) = stable_pair(&h.bank);
    h.store.put_stable_entry(&seeded, "tester").unwrap();
    let bundle = write_bundle(
        &ExportEntities {
            persona_stable: vec![bundled],
            ..ExportEntities::default()
        },
        ExportContentMode::DsarPlaintext,
    );
    let report = import(&h.args(&h.ctx(), h.policy(), bundle.path(), false, false, None))
        .expect("non-force import succeeds");
    assert_eq!(report.would_skip, 1);
    let history = h
        .store
        .stable_history(&h.bank, "communication_style")
        .unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].context.as_deref(), Some("old why"));

    let h2 = Harness::new();
    let (seeded_pref, bundled_pref) = preference_pair(&h2.bank);
    h2.store
        .put_preference_state(&seeded_pref, "tester")
        .unwrap();
    let bundle_forced = write_bundle(
        &ExportEntities {
            persona_preferences: vec![bundled_pref],
            ..ExportEntities::default()
        },
        ExportContentMode::DsarPlaintext,
    );
    let report_forced = import(&h2.args(
        &h2.ctx(),
        h2.policy(),
        bundle_forced.path(),
        false,
        false,
        None,
    ))
    .expect("non-force import succeeds");
    assert_eq!(report_forced.would_skip, 1);
    let stored = h2
        .store
        .get_preference(&h2.bank, "brevity_affinity")
        .unwrap()
        .expect("preference exists");
    assert_eq!(stored.context.as_deref(), Some("old pref why"));
}

#[test]
fn identical_context_import_stays_idempotent() {
    let h = Harness::new();
    let (seeded, mut bundled) = stable_pair(&h.bank);
    bundled.context.clone_from(&seeded.context);
    h.store.put_stable_entry(&seeded, "tester").unwrap();
    let bundle = write_bundle(
        &ExportEntities {
            persona_stable: vec![bundled],
            ..ExportEntities::default()
        },
        ExportContentMode::DsarPlaintext,
    );
    let report = import(&h.args(
        &h.ctx(),
        h.policy(),
        bundle.path(),
        false,
        true,
        Some("restore"),
    ))
    .expect("identical-context force import succeeds");
    assert_eq!(report.would_skip, 1, "identical row must stay deduplicated");
    assert_eq!(report.would_overwrite, 0);
    let history = h
        .store
        .stable_history(&h.bank, "communication_style")
        .unwrap();
    assert_eq!(history.len(), 1, "no duplicate version may be created");

    let h2 = Harness::new();
    let (seeded_pref, mut bundled_pref) = preference_pair(&h2.bank);
    bundled_pref.context.clone_from(&seeded_pref.context);
    h2.store
        .put_preference_state(&seeded_pref, "tester")
        .unwrap();
    let bundle_forced = write_bundle(
        &ExportEntities {
            persona_preferences: vec![bundled_pref],
            ..ExportEntities::default()
        },
        ExportContentMode::DsarPlaintext,
    );
    let report_forced = import(&h2.args(
        &h2.ctx(),
        h2.policy(),
        bundle_forced.path(),
        false,
        true,
        Some("restore"),
    ))
    .expect("identical-context force import succeeds");
    assert_eq!(report_forced.would_skip, 1);
    assert_eq!(report_forced.would_overwrite, 0);
}
