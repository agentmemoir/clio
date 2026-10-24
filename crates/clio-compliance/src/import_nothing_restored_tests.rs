// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the report a live import produces when it
//! restores nothing because the target bank already holds every entity, and when
//! it restores nothing because every row it carried was skipped as invalid: the
//! run says which, instead of reading like a successful restore.
//!
//! ## Owns
//! - The "nothing restored" note on a zero-create, zero-overwrite live run
//!   whose rows were skipped as invalid
//!
//! ## Does not own
//! - Fixture builders (see `import_fixture.rs`)
//! - The import report shape (see `import.rs`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::bundle::ExportContentMode;
use crate::import::import;
use crate::import_fixture::{Harness, write_bundle};

#[test]
fn import_that_creates_nothing_says_so() {
    let h = Harness::new();
    let bundle = write_bundle(&h.full_entities(), ExportContentMode::DsarPlaintext);
    let ctx = h.ctx();
    let first = import(&h.args(&ctx, h.policy(), bundle.path(), false, false, None))
        .expect("the first import restores the bundle");
    assert_eq!(first.would_create, 11, "every entity is created once");
    assert!(
        !first.notes.iter().any(|n| n.contains("nothing restored")),
        "a restoring run is not a no-op: {:?}",
        first.notes
    );

    let second = import(&h.args(&ctx, h.policy(), bundle.path(), false, false, None))
        .expect("re-importing an already-restored bundle succeeds");

    assert_eq!(second.would_create, 0, "every entity already exists");
    assert_eq!(second.would_overwrite, 0);
    assert!(second.would_skip > 0, "the rows are skipped, not rejected");
    assert!(second.created_ids.is_empty(), "nothing was created");
    assert!(
        second.applied,
        "applied keeps its meaning: the live run reached the batch commit"
    );
    let skipped = format!("{} skipped", second.would_skip);
    assert!(
        second
            .notes
            .iter()
            .any(|n| n.contains("nothing restored") && n.contains(&skipped)),
        "the run says what it skipped: {:?}",
        second.notes
    );
}

#[test]
fn import_whose_rows_are_all_invalid_counts_them_in_the_nothing_restored_note() {
    let h = Harness::new();
    let mut entities = h.full_entities();
    // Item rows only: every other kind stages a write, which would mask the note.
    entities.triples.clear();
    entities.assoc_edges.clear();
    entities.persona_stable.clear();
    entities.persona_preferences.clear();
    entities.tasks.clear();
    entities.failures.clear();
    entities.beliefs.clear();
    for item in &mut entities.items {
        item.gist = Some("g".repeat(1079));
    }
    let bundle = write_bundle(&entities, ExportContentMode::DsarPlaintext);
    let report = import(&h.args(&h.ctx(), h.policy(), bundle.path(), false, false, None))
        .expect("a bundle of over-long gists does not abort the import");

    assert_eq!(report.would_create, 0, "no row is created");
    assert_eq!(report.would_overwrite, 0, "no row is overwritten");
    assert_eq!(report.would_skip_invalid, entities.items.len());
    let note = report
        .notes
        .iter()
        .find(|note| note.contains("nothing restored"))
        .unwrap_or_else(|| {
            panic!(
                "an all-invalid run says it restored nothing: {:?}",
                report.notes
            )
        });
    let counted = format!("{} skipped as invalid", report.would_skip_invalid);
    assert!(
        note.contains(&counted),
        "the note carries the invalid count, not zero: {note}"
    );
    assert!(
        !note.contains("0 skipped as invalid"),
        "an all-invalid run never reports zero invalid rows: {note}"
    );
}
