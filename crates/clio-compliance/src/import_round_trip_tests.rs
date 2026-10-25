// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the round trip between the two sides of the
//! bundle contract: a live bank written by the real export writer restores into
//! a second bank through the real import entrypoint, and the bundle file format
//! itself accepts a gist longer than the published cap.
//!
//! ## Owns
//! - The live-bank export-then-import restore assertion
//! - The over-cap gist serialization and checksum assertion
//!
//! ## Does not own
//! - Fixture builders (see `export_tests.rs` and `import_fixture.rs`)
//! - What the import does with an over-cap gist (see `import_gist_bound_tests.rs`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use clio_store::Store;
use clio_types::{Actor, GIST_MAX_CHARS, SemanticCategory};

use crate::bundle::{
    ExportContentMode, ExportEntities, canonical_value_bytes, entities_checksum, parse_bundle,
    sha256_hex_bytes,
};
use crate::export::{ExportFilter, export};
use crate::export_tests as export_fixtures;
use crate::import::import;
use crate::import_fixture::{Harness, unique, write_bundle};

/// The gists a live bank holds: realistic summaries, each inside the cap.
const LIVE_GISTS: [&str; 4] = [
    "The smoke task checklist runs before every tagged release",
    "Deploys go through the staging bank before production",
    "Rollbacks reuse the previous bundle instead of a rebuild",
    "The on-call engineer owns the import drill each quarter",
];

/// A bank that only the public write path filled is exported through the real
/// writer, and every item comes back in a second bank with its gist intact.
#[test]
fn export_then_import_restores_a_live_bank() {
    let source = export_fixtures::store();
    let source_bank = unique("round-trip-source");
    let mut seeded: Vec<(String, String)> = Vec::new();
    for gist in LIVE_GISTS {
        let id = unique("rt-itm");
        let mut item = export_fixtures::item(
            &id,
            &source_bank,
            "sub-round-trip",
            Some(SemanticCategory::parse("schema").unwrap()),
            None,
            Some(serde_json::json!({ "step": gist })),
        );
        item.gist = Some(gist.to_owned());
        source
            .create_memory_item(&item, "tester")
            .expect("a gist inside the published cap is storable");
        seeded.push((id, gist.to_owned()));
    }
    let path = export_fixtures::dest_path("import-round-trip");
    let fixture = export_fixtures::Fixture {
        store: source,
        bank: source_bank,
        path,
    };
    let export_ctx = export_fixtures::ctx(&fixture, Actor::Harness);
    let outcome = export(&export_fixtures::args(
        &fixture,
        &export_ctx,
        ExportFilter::default(),
        ExportContentMode::DsarPlaintext,
    ))
    .expect("a live bank exports");
    assert!(
        outcome.manifest.complete,
        "a whole-bank export omits nothing"
    );

    let target = Harness::new();
    let report = import(&target.args(
        &target.ctx(),
        target.policy(),
        fixture.path.as_str(),
        false,
        false,
        None,
    ))
    .expect("the exported bundle imports");

    assert_eq!(
        report.would_create,
        LIVE_GISTS.len(),
        "every exported item is created in the target bank"
    );
    assert_eq!(report.would_skip, 0);
    assert_eq!(
        report.would_skip_invalid, 0,
        "no exported gist trips the cap"
    );
    for (id, gist) in seeded {
        let restored_id = format!("{id}#{}", target.bank);
        assert!(
            report.created_ids.contains(&restored_id),
            "the report names the created id {restored_id}: {:?}",
            report.created_ids
        );
        let restored = target
            .store
            .get_memory_item(&target.bank, &restored_id)
            .expect("read the restored item")
            .expect("the restored item exists");
        assert_eq!(
            restored.gist.as_deref(),
            Some(gist.as_str()),
            "gist restored for {restored_id}"
        );
    }
}

/// The file contract carries a gist longer than the cap verbatim and its
/// checksum still verifies, so the bundle format is not what rejects it.
#[test]
fn over_cap_gist_survives_bundle_serialization() {
    let over_cap = "g".repeat(1079);
    assert!(over_cap.chars().count() > GIST_MAX_CHARS);
    let mut item = export_fixtures::item(
        &unique("gist-over-cap"),
        "exported-bank",
        "sub-over-cap",
        Some(SemanticCategory::parse("schema").unwrap()),
        None,
        None,
    );
    item.gist = Some(over_cap.clone());
    let bundle = write_bundle(
        &ExportEntities {
            items: vec![item],
            ..ExportEntities::default()
        },
        ExportContentMode::DsarPlaintext,
    );

    let raw: serde_json::Value =
        serde_json::from_str(&bundle.text()).expect("the written file is JSON");
    let parsed = parse_bundle(&raw).expect("the file contract accepts a legacy gist");
    assert_eq!(
        parsed
            .envelope
            .entities
            .items
            .first()
            .and_then(|it| it.gist.as_deref()),
        Some(over_cap.as_str()),
        "the gist is written and read back verbatim, never truncated"
    );

    let manifest = &parsed.envelope.manifest;
    assert_eq!(
        entities_checksum(&parsed.envelope.entities),
        manifest.content_sha256,
        "the typed payload matches the manifest checksum"
    );
    assert_eq!(
        sha256_hex_bytes(&canonical_value_bytes(&parsed.raw_entities)),
        manifest.content_sha256,
        "the checksum verifies over the payload as parsed from the file"
    );
}
