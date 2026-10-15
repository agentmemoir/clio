// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the export-fixture-backed import restore
//! tests: a content-free `ciphertext_backup` bundle still importing, and a
//! bundle whose `context` was dropped by the scrub bound still importing.
//!
//! ## Owns
//! - Restore assertions that drive the export fixtures end to end
//!
//! ## Does not own
//! - The shared import harness (see `import_fixture.rs`) and the export fixtures
//!   (see `export_tests.rs`)
//! - Pure import restore assertions (see `import_restore_tests.rs`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use clio_store::{PersonaStore, Store};
use clio_types::{Actor, CONTEXT_MAX_BYTES, SemanticCategory};

use crate::bundle::ExportContentMode;
use crate::export::{ExportFilter, export as export_bundle};
use crate::export_tests as export_fixtures;
use crate::import::import;
use crate::import_fixture::{Harness, unique};

/// A content-free `ciphertext_backup` bundle still passes manifest
/// verification and imports; the persona metadata inventory restores.
#[test]
fn ciphertext_backup_bundle_with_domain_records_still_imports() {
    let f = export_fixtures::fixture("backup-domain");
    let export_ctx = export_fixtures::ctx(&f, Actor::Harness);
    let outcome = export_bundle(&export_fixtures::args(
        &f,
        &export_ctx,
        ExportFilter::default(),
        ExportContentMode::CiphertextBackup,
    ))
    .expect("export succeeds");
    assert!(outcome.manifest.complete);
    let counts = &outcome.manifest.counts_by_type;
    let counted = counts
        .iter()
        .find(|(label, _)| label == "persona_preferences");
    assert_eq!(
        counted.map(|(_, count)| *count),
        Some(1),
        "manifest counts the payload"
    );

    let h = Harness::new();
    let report = import(&h.args(&h.ctx(), h.policy(), &f.dest, false, false, None))
        .expect("a content-free backup bundle must still import");
    assert!(report.applied);
    assert_eq!(report.would_overwrite, 0);
    assert!(
        h.store
            .get_preference(&h.bank, "brevity_affinity")
            .unwrap()
            .is_some()
    );
    let history = h
        .store
        .stable_history(&h.bank, "communication_style")
        .unwrap();
    assert!(!history.is_empty(), "{history:?}");
}

/// a context the scrub pushed over the published maximum is dropped by
/// the export writer — never written over-long — and the bundle still imports;
/// the value is simply absent on restore.
#[test]
fn bundle_with_a_context_dropped_by_the_scrub_bound_still_imports() {
    let store = export_fixtures::store();
    let bank = unique("ctx-bound-bank");
    let id = unique("ctx-bound-item");
    let expanding = format!("{}token: a", "x".repeat(CONTEXT_MAX_BYTES - 8));
    let mut item = export_fixtures::item(
        &id,
        &bank,
        "sub-bound",
        Some(SemanticCategory::parse("schema").unwrap()),
        None,
        None,
    );
    item.context = Some(expanding);
    store
        .create_memory_item(&item, "tester")
        .expect("seed an expandable context");

    let path = export_fixtures::dest_path("ctx-bound");
    let f = export_fixtures::Fixture {
        store,
        bank,
        dest: path.as_str().to_owned(),
        path,
    };
    let export_ctx = export_fixtures::ctx(&f, Actor::Harness);
    export_bundle(&export_fixtures::args(
        &f,
        &export_ctx,
        ExportFilter::default(),
        ExportContentMode::DsarPlaintext,
    ))
    .expect("export succeeds");
    let text = std::fs::read_to_string(f.path.as_path()).expect("bundle readable");
    assert!(
        !text.contains("[REDACTED]"),
        "the expanded value must not be written"
    );
    assert!(
        !text.contains("\"context\""),
        "the dropped context must not be serialized"
    );

    let h = Harness::new();
    import(&h.args(&h.ctx(), h.policy(), &f.dest, false, false, None))
        .expect("a bundle with a dropped context still imports");
    let restored = h
        .store
        .get_memory_item(&h.bank, &id)
        .expect("read restored item")
        .expect("restored item exists");
    assert!(restored.context.is_none());
}
