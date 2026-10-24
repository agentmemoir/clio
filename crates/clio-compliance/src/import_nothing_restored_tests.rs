// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the report a live import produces when it
//! restores nothing because the target bank already holds every entity: the run
//! says so in its notes instead of reading like a successful restore.
//!
//! ## Owns
//! - The "nothing restored" note on a zero-create, zero-overwrite live run
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
