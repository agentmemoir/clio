// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the bundle import restore-surface tests:
//! force overwrites across entity kinds (and the `import_force` audit rows they
//! stage), opaque-envelope restore, carrier planning notes, belief trajectory
//! restore, skip-without-force behavior, and content-mode bundle restore.
//!
//! ## Owns
//! - Restore-path import assertions over the shared `import_fixture` harness
//! - The `import_force` audit assertions over the `AuditSpy` batch boundary
//!
//! ## Does not own
//! - Fixture builders (see `import_fixture.rs`, and the export-side seeds in
//!   `export_tests.rs`)
//! - Validation/ordering tests (see `import_tests.rs`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use clio_admission::AdmissionPolicy;
use clio_config::ranking::RankingEnv;
use clio_store::{BeliefStore, PersonaStore, Store};
use clio_types::{ErrorCode, OpContext};

use crate::bundle::ExportContentMode;
use crate::import::{ImportArgs, ImportReport, import};
use crate::import_fixture::{Harness, count_items, entry, unique, write_bundle};
use crate::import_fixture_signals::{AuditSpy, EmptySignals};

#[test]
#[allow(clippy::float_cmp)] // exact restored values from the bundle payload
fn force_overwrites_differing_assoc_persona_rows() {
    let h = Harness::new();
    let entities = h.full_entities();
    let bundle = write_bundle(&entities, ExportContentMode::DsarPlaintext);
    let ctx = h.ctx();
    import(&h.args(&ctx, h.policy(), bundle.path(), false, false, None)).unwrap();

    // Differing assoc weight, stable value, and preference value.
    let mut modified = entities;
    modified.assoc_edges[0].weight = 0.9;
    modified.persona_stable[0].value = "terse".into();
    modified.persona_preferences[0].value = 0.9;
    let bundle = write_bundle(&modified, ExportContentMode::DsarPlaintext);
    let report = import(&h.args(&ctx, h.policy(), bundle.path(), false, true, Some("ops")))
        .expect("force import succeeds");
    assert_eq!(report.would_overwrite, 7); // 4 items + assoc + stable + pref
    assert_eq!(report.would_skip, 4); // triple + task record + failure record + belief
    assert_eq!(
        h.store.list_assoc_edges(&h.bank, None).unwrap()[0].weight,
        0.9
    );
    let pref = h
        .store
        .get_preference(&h.bank, "brevity_affinity")
        .unwrap()
        .unwrap();
    assert_eq!(pref.value, 0.9);
}

#[test]
fn force_overwrites_opaque_envelope() {
    let h = Harness::new();
    let entities = h.opaque_entities();
    let first = write_bundle(&entities, ExportContentMode::CiphertextBackup);
    let ctx = h.ctx();
    import(&h.args(&ctx, h.policy(), first.path(), false, false, None)).unwrap();

    let mut modified = entities;
    modified.opaque_items[0].content_cipher = "rotated-ciphertext".into();
    let bundle = write_bundle(&modified, ExportContentMode::CiphertextBackup);
    let report = import(&h.args(&ctx, h.policy(), bundle.path(), false, true, Some("ops")))
        .expect("force opaque import succeeds");
    assert_eq!(report.would_overwrite, 1);
    assert_eq!(
        h.store
            .inspect_content_ciphertext(
                &h.bank,
                &format!("{}#{}", modified.opaque_items[0].id, h.bank)
            )
            .unwrap()
            .as_deref(),
        Some("rotated-ciphertext")
    );
}

#[test]
fn tombstones_are_skipped_as_export_provenance() {
    let h = Harness::new();
    let bundle = write_bundle(
        &Harness::tombstone_entities(),
        ExportContentMode::DsarPlaintext,
    );
    let ctx = h.ctx();
    let report = import(&h.args(&ctx, h.policy(), bundle.path(), false, false, None))
        .expect("tombstone import succeeds");
    assert_eq!(report.would_skip, 1);
    assert!(
        report
            .notes
            .iter()
            .any(|n| n.contains("tombstones are export provenance"))
    );
    assert_eq!(count_items(&h), 0);
}

#[test]
fn missing_carrier_notes_truncate() {
    let h = Harness::new();
    let bundle = write_bundle(
        &h.missing_carrier_entities(11),
        ExportContentMode::DsarPlaintext,
    );
    let ctx = h.ctx();
    let report = import(&h.args(&ctx, h.policy(), bundle.path(), true, false, None))
        .expect("missing-carrier planning succeeds");
    assert_eq!(report.would_skip, 11);
    assert_eq!(report.would_create, 0);
    assert_eq!(report.notes.len(), 10);
    assert_eq!(
        report.notes.last().map(String::as_str),
        Some("(further notes truncated)")
    );
    assert!(report.notes[0].contains("carrier item missing from bundle"));
}

#[test]
fn carrier_already_exists_skips_with_note() {
    let h = Harness::new();
    let base = h.full_entities();
    import(&h.args(
        &h.ctx(),
        h.policy(),
        write_bundle(&base, ExportContentMode::DsarPlaintext).path(),
        false,
        false,
        None,
    ))
    .unwrap();

    // A NEW edge id reusing an existing carrier item id.
    let mut modified = base;
    modified.triples[0].id = unique("trp");
    let bundle = write_bundle(&modified, ExportContentMode::DsarPlaintext);
    let report = import(&h.args(&h.ctx(), h.policy(), bundle.path(), false, false, None))
        .expect("conflict planning succeeds");
    assert!(
        report
            .notes
            .iter()
            .any(|n| n.contains("carrier item already exists"))
    );
    assert_eq!(h.store.list_triples(&h.bank).unwrap().len(), 1);
}

#[test]
fn belief_trajectory_dedupes_and_appends() {
    let h = Harness::new();
    let ctx = h.ctx();
    let e1 = entry("2026-01-01", 0.7, "turn_1");
    let e2 = entry("2026-02-01", 0.8, "turn_2");
    let e3 = entry("2026-03-01", 0.9, "turn_3");

    // Two-entry trajectory: identity create plus the appended second entry.
    let two = write_bundle(
        &h.belief_entities(vec![e1.clone(), e2]),
        ExportContentMode::DsarPlaintext,
    );
    let report = import(&h.args(&ctx, h.policy(), two.path(), false, false, None))
        .expect("belief import succeeds");
    assert_eq!(report.would_create, 2);
    let beliefs = h.store.list_beliefs(&h.bank).unwrap();
    assert_eq!((beliefs.len(), beliefs[0].confidence_history.len()), (1, 2));

    // Re-import the same trajectory: fully deduped.
    let again = import(&h.args(&ctx, h.policy(), two.path(), false, false, None)).unwrap();
    assert_eq!(again.would_skip, 1);
    assert_eq!(again.would_create, 0);

    // One new entry appends exactly one row (validated-only, no re-score).
    let three = write_bundle(
        &h.belief_entities(vec![e1, e3]),
        ExportContentMode::DsarPlaintext,
    );
    let report = import(&h.args(&ctx, h.policy(), three.path(), false, false, None))
        .expect("belief append succeeds");
    assert_eq!(report.would_create, 1);
    let beliefs = h.store.list_beliefs(&h.bank).unwrap();
    assert_eq!(beliefs[0].confidence_history.len(), 3);
}

#[test]
fn belief_create_rejected_below_threshold() {
    let h = Harness::new();
    let entities = h.belief_entities(vec![entry("2026-01-01", 0.7, "turn_1")]);
    let bundle = write_bundle(&entities, ExportContentMode::DsarPlaintext);
    let ctx = h.ctx();
    let mut env = RankingEnv::system_defaults();
    env.theta_admit = 1.0;
    let policy = AdmissionPolicy::new(&env);
    let report = import(&h.args(&ctx, policy, bundle.path(), false, false, None))
        .expect("belief reject run succeeds");
    assert_eq!(report.would_reject, 1);
    let beliefs = h.store.list_beliefs(&h.bank).unwrap();
    assert!(beliefs.is_empty(), "{beliefs:?}");
}

#[test]
fn differing_existing_rows_skip_without_force() {
    let h = Harness::new();
    let entities = h.full_entities();
    import(&h.args(
        &h.ctx(),
        h.policy(),
        write_bundle(&entities, ExportContentMode::DsarPlaintext).path(),
        false,
        false,
        None,
    ))
    .unwrap();

    let mut modified = entities;
    modified.persona_stable[0].value = "terse".into();
    modified.persona_preferences[0].value = 0.1;
    modified.opaque_items = h.opaque_entities().opaque_items;
    let bundle = write_bundle(&modified, ExportContentMode::DsarPlaintext);
    let report = import(&h.args(&h.ctx(), h.policy(), bundle.path(), false, false, None))
        .expect("no-force re-import succeeds");
    assert_eq!(report.would_overwrite, 0);
    let opaque_id = format!("{}#{}", modified.opaque_items[0].id, h.bank);
    assert_eq!(
        h.store
            .inspect_content_ciphertext(&h.bank, &opaque_id)
            .unwrap()
            .as_deref(),
        Some("opaque-at-rest-ciphertext-from-exporting-node")
    );
}

#[test]
fn episodic_carrier_restores_through_the_gate() {
    let h = Harness::new();
    let entities = h.temporal_entities();
    let bundle = write_bundle(&entities, ExportContentMode::DsarPlaintext);
    let ctx = h.ctx();
    let report = import(&h.args(&ctx, h.policy(), bundle.path(), false, false, None))
        .expect("temporal item import succeeds");
    assert_eq!(report.would_create, 1);
    assert!(
        h.store
            .get_memory_item(&h.bank, &format!("{}#{}", entities.items[0].id, h.bank))
            .unwrap()
            .is_some()
    );
}

#[test]
fn belief_dry_run_rejects_below_threshold_without_writes() {
    let h = Harness::new();
    let entities = h.belief_entities(vec![entry("2026-01-01", 0.7, "turn_1")]);
    let bundle = write_bundle(&entities, ExportContentMode::DsarPlaintext);
    let ctx = h.ctx();
    let mut env = RankingEnv::system_defaults();
    env.theta_admit = 1.0;
    let policy = AdmissionPolicy::new(&env);
    let report = import(&h.args(&ctx, policy, bundle.path(), true, false, None))
        .expect("dry-run belief reject succeeds");
    assert_eq!(report.would_reject, 1);
    let beliefs = h.store.list_beliefs(&h.bank).unwrap();
    assert!(beliefs.is_empty(), "{beliefs:?}");
}

#[test]
fn belief_with_empty_history_is_rejected() {
    let h = Harness::new();
    let bundle = write_bundle(&h.belief_entities(vec![]), ExportContentMode::DsarPlaintext);
    let ctx = h.ctx();
    let err = import(&h.args(&ctx, h.policy(), bundle.path(), false, false, None)).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
}

/// Import args whose batch boundary is `spy`, so the `import_force` audit rows
/// staged by the planner can be observed (see [`AuditSpy`]).
fn spy_import<'a>(
    h: &'a Harness,
    spy: &'a AuditSpy<'a>,
    ctx: &'a OpContext,
    source: &'a str,
    force: bool,
    reason: Option<&'a str>,
) -> ImportReport {
    import(&ImportArgs {
        store: &h.store,
        batch: spy,
        persona: &h.store,
        history: &h.store,
        belief: &h.store,
        ctx,
        source,
        dry_run: false,
        force,
        reason,
        policy: h.policy(),
        signals: &EmptySignals,
        sink: &h.admit,
        belief_signals: &EmptySignals,
        timestamp: "2026-06-01T00:00:00Z",
    })
    .expect("import succeeds")
}

/// A persona force overwrite stages an `import_force` audit row naming
/// the persona handle and carrying the caller's reason; the create and skip
/// paths stage none.
#[test]
fn persona_force_overwrite_stages_import_force_audit() {
    let h = Harness::new();
    let entities = h.full_entities();
    let spy = AuditSpy::new(&h.store);
    let ctx = h.ctx();
    let bundle = write_bundle(&entities, ExportContentMode::DsarPlaintext);
    assert_eq!(
        spy_import(&h, &spy, &ctx, bundle.path(), false, None).would_create,
        11
    );
    assert!(
        spy.audits().is_empty(),
        "a plain create must not audit a force overwrite"
    );

    let mut modified = entities;
    modified.persona_stable[0].value = "terse".into();
    modified.persona_preferences[0].value = 0.9;
    let modified = write_bundle(&modified, ExportContentMode::DsarPlaintext);
    assert_eq!(
        spy_import(&h, &spy, &ctx, modified.path(), false, None).would_overwrite,
        0
    );
    assert!(
        spy.audits().is_empty(),
        "a skip must not audit a force overwrite"
    );

    let forced = spy_import(&h, &spy, &ctx, modified.path(), true, Some("ops restore"));
    assert!(forced.would_overwrite >= 2, "the persona rows overwrite");
    for handle in [
        "persona-stable:communication_style",
        "persona-preference:brevity_affinity",
    ] {
        assert!(
            spy.audits()
                .iter()
                .any(|row| row.operation == "import_force"
                    && row.item_id.as_deref() == Some(handle)
                    && row.reason.as_deref() == Some("ops restore")),
            "no `import_force` audit row for {handle}"
        );
    }
}

#[cfg(test)]
#[path = "import_restore_tests_export_tests.rs"]
mod import_restore_export_tests;

#[cfg(test)]
#[path = "import_restore_tests_backup_restore_tests.rs"]
mod backup_restore_tests;

#[cfg(test)]
#[path = "import_restore_tests_persona_scrub_tests.rs"]
mod import_persona_scrub_tests;
