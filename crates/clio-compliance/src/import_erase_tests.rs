// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests imports after subject erasure.
//!
//! ## Owns
//! - Opaque backup import refusal for erased subjects
//!
//! ## Does not own
//! - Shared import fixtures (see `import_fixture.rs`)
//!
//! ## Boundary
//!
//! Changes to erased-subject import behavior should normally require changes here.
//! Changes to import planning or fixture construction should not require changes here.
//!
//! Keep this module focused on erased-subject import behavior.

use clio_store::{ComplianceStore, ItemInspectFilter, Store, SubjectEraseSpec};
use clio_types::ErrorCode;

use crate::bundle::ExportContentMode;
use crate::import::import;
use crate::import_fixture::{Harness, write_bundle};

#[test]
fn ciphertext_backup_import_rejects_erased_subject() {
    let h = Harness::new();
    let entities = h.opaque_entities();
    let opaque_id = entities.opaque_items[0].id.clone();
    let bundle = write_bundle(&entities, ExportContentMode::CiphertextBackup);

    let mut seed = h
        .full_entities()
        .items
        .into_iter()
        .next()
        .expect("fixture has a subject item");
    seed.bank_id = h.bank.clone();
    h.store
        .create_memory_item(&seed, "test")
        .expect("seed subject before erasure");
    let salt = [19; 16];
    h.store
        .erase_subject(&SubjectEraseSpec {
            bank_id: &h.bank,
            subject_id: &h.subject,
            request_id: "opaque-import-erasure",
            legal_basis: "GDPR_ART_17",
            reason_code: "COMPLIANCE_CRYPTO_SHRED",
            deleted_at: "2026-06-01T00:00:00Z",
            salt: &salt,
        })
        .expect("erase subject");

    let ctx = h.ctx();
    let result = import(&h.args(&ctx, h.policy(), bundle.path(), false, false, None));

    assert_eq!(result.unwrap_err().code, ErrorCode::ErasedSubject);
    let rows = h
        .store
        .inspect_items(&ItemInspectFilter::new(&h.bank))
        .unwrap();
    assert!(
        rows.iter().all(|row| row.item_id != opaque_id),
        "refused opaque item has no row"
    );
}
