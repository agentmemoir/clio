// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! Test-only units owned by `journal_row.rs`: the journal content identity
//! that pairs `(bank_id, sync_mutation_id)`. Never compiled into production
//! builds.

use clio_store::{DekProvider, LocalDevKms, composite_row_id};
use clio_types::ErrorCode;

#[test]
fn journal_content_is_bound_to_one_mutation() {
    let kms = LocalDevKms::new();
    let first = composite_row_id(&["bank-1", "mutation-a"]);
    let second = composite_row_id(&["bank-1", "mutation-b"]);
    let envelope = kms
        .encrypt("journal.content", &first, "sub-1", b"payload")
        .expect("seal");

    assert_eq!(
        kms.decrypt("journal.content", &first, &envelope)
            .expect("open same mutation"),
        b"payload"
    );
    assert_eq!(
        kms.decrypt("journal.content", &second, &envelope)
            .unwrap_err()
            .code,
        ErrorCode::Forbidden,
        "a mutation must not open another mutation's content"
    );
}

#[test]
fn journal_content_is_bound_to_its_bank() {
    let kms = LocalDevKms::new();
    let first = composite_row_id(&["bank-1", "mutation-a"]);
    let other_bank = composite_row_id(&["bank-2", "mutation-a"]);
    let envelope = kms
        .encrypt("journal.content", &first, "sub-1", b"payload")
        .expect("seal");

    assert_eq!(
        kms.decrypt("journal.content", &other_bank, &envelope)
            .unwrap_err()
            .code,
        ErrorCode::Forbidden
    );
}
