// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the import behaviour a stored `gist` that
//! predates the published length cap requires: the row is reported and skipped
//! with its entity id, and every other entity in the bundle still applies.
//!
//! ## Owns
//! - The oversized-gist skip on the create path and on the force path
//! - The `skipped_invalid` report entries those skips produce
//!
//! ## Does not own
//! - Fixture builders (see `import_fixture.rs`)
//! - Structural `context` violations, which still abort (see
//!   `import_context_tests.rs`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::bundle::ExportContentMode;
use crate::import::import;
use crate::import_fixture::{Harness, unique, write_bundle};
use clio_store::Store;
use clio_types::{EpistemicKind, ItemKind, MemoryItem, SemanticCategory};

const SHORT_GIST: &str = "The deploy must run the smoke task checklist";

/// One hand-built semantic item carrying `gist`, as a bundle file would.
fn item(id: &str, gist: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: "exported-bank".into(),
        subject_id: "sub-gist-bound".into(),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::parse("schema").unwrap()),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.9),
        admission_score: Some(0.8),
        source_ref: None,
        snapshot: Some(serde_json::json!({ "k": "v" })),
        gist: Some(gist.into()),
        valid_from: "2026-01-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2026-01-01T00:00:00Z".into(),
        tx_until: None,
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-01-01T00:00:00Z".into(),
        context: None,
    }
}

/// The id the import writes `id` under: the bundle bank differs from the target
/// bank, so every row is re-keyed.
fn written(h: &Harness, id: &str) -> String {
    format!("{id}#{}", h.bank)
}

/// A row the target bank holds after `import`.
fn stored(h: &Harness, id: &str) -> Option<MemoryItem> {
    h.store
        .get_memory_item(&h.bank, &written(h, id))
        .expect("store read")
}

#[test]
fn legacy_oversized_gist_is_skipped_with_reported_id_not_aborted() {
    let h = Harness::new();
    let offending = unique("gist-legacy");
    let valid = unique("gist-valid");
    let bundle = write_bundle(
        &crate::bundle::ExportEntities {
            items: vec![
                item(&offending, &"g".repeat(1079)),
                item(&valid, SHORT_GIST),
            ],
            ..crate::bundle::ExportEntities::default()
        },
        ExportContentMode::DsarPlaintext,
    );
    let report = import(&h.args(&h.ctx(), h.policy(), bundle.path(), false, false, None))
        .expect("a legacy oversized gist must not abort the import");

    assert_eq!(report.would_create, 1, "only the valid row is created");
    assert_eq!(report.would_skip_invalid, 1);
    assert_eq!(report.would_skip, 0, "an invalid row is not a skip");
    assert_eq!(report.would_reject, 0, "an invalid row is not a reject");
    let skipped = report.skipped_invalid.first().expect("one skipped entity");
    assert_eq!(skipped.id, written(&h, &offending));
    assert_eq!(skipped.code, "out_of_range");
    assert!(
        skipped.reason.contains("250") && skipped.reason.contains("1079"),
        "reason names the limit and the length: {}",
        skipped.reason
    );
    assert!(stored(&h, &valid).is_some(), "the valid row is written");
    assert!(
        stored(&h, &offending).is_none(),
        "the offending row is never written"
    );
}

#[test]
fn force_with_legacy_oversized_gist_skips_that_row_and_overwrites_the_rest() {
    let h = Harness::new();
    let offending = unique("gist-force-bad");
    let valid = unique("gist-force-good");
    let entities = |first: &str, second: &str| crate::bundle::ExportEntities {
        items: vec![item(&offending, first), item(&valid, second)],
        ..crate::bundle::ExportEntities::default()
    };
    let clean = write_bundle(
        &entities(SHORT_GIST, SHORT_GIST),
        ExportContentMode::DsarPlaintext,
    );
    import(&h.args(&h.ctx(), h.policy(), clean.path(), false, false, None))
        .expect("the clean bundle imports");

    let oversized = "g".repeat(1079);
    let bad = write_bundle(
        &entities(&oversized, "rewritten by force"),
        ExportContentMode::DsarPlaintext,
    );
    let report = import(&h.args(
        &h.ctx(),
        h.policy(),
        bad.path(),
        false,
        true,
        Some("ops restore"),
    ))
    .expect("a force import must not fail on a legacy gist");

    assert!(report.applied, "the run reached the batch commit");
    assert_eq!(report.would_overwrite, 1, "the valid row is overwritten");
    assert_eq!(report.would_skip_invalid, 1);
    let kept = stored(&h, &offending).expect("the existing row survives");
    assert_eq!(kept.gist.as_deref(), Some(SHORT_GIST), "never overwritten");
    let overwritten = stored(&h, &valid).expect("the valid row exists");
    assert_eq!(overwritten.gist.as_deref(), Some("rewritten by force"));
    let trail = h
        .store
        .audit_trail(&h.bank, &written(&h, &valid))
        .expect("audit trail");
    assert!(
        trail.events.iter().any(|e| e.operation == "import_force"),
        "the overwrite is audited: {:?}",
        trail.events
    );
}
