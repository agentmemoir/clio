// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! This module checks that correction refuses a shredded subject atomically.
//!
//! Test-only. Never compiled into production builds.

use crate::audit_model::CorrectionSpec;
use crate::db_url::postgres_url_from_env;
use crate::erase_test_support::{local_kms, memory_item, unique};
use crate::pg_lock::lock_pg_test;
use crate::postgres::PostgresStore;
use crate::store::Store;
use clio_types::ErrorCode;

#[test]
fn pg_correct_erased_subject_refuses_and_rolls_back_prior() {
    let _guard = lock_pg_test();
    let url = postgres_url_from_env();
    let kms = local_kms();
    let store = match PostgresStore::open(url.as_str(), kms) {
        Ok(store) => store,
        Err(err) => {
            crate::pg_skip::unreachable_pg_or_skip("correct erased-subject guard", &err);
            return;
        }
    };
    let bank = unique("correct-erased-bank");
    let subject = unique("correct-erased-subject");
    let prior_id = unique("correct-erased-prior");
    let new_id = unique("correct-erased-new");
    let prior = memory_item(&bank, &prior_id, &subject);
    store.create_memory_item(&prior, "tester").unwrap();

    // Keep the row and KMS key while setting the DB tombstone, so correction
    // reaches the store guard and can prove its prior-row update rolls back.
    let erased_at = "2026-10-17T00:00:00Z";
    store
        .test_execute(&format!(
            "UPDATE subject_keys SET dek_ref = 'shredded', status = 'shredded', \
             shredded_at = '{erased_at}', updated_at = '{erased_at}' \
             WHERE subject_id = '{subject}'"
        ))
        .unwrap();

    let mut new_item = prior.clone();
    new_item.id.clone_from(&new_id);
    new_item.valid_from = erased_at.into();
    new_item.tx_from = erased_at.into();
    new_item.created_at = erased_at.into();
    new_item.updated_at = erased_at.into();
    let err = store
        .correct_item(&CorrectionSpec {
            bank_id: bank.clone(),
            prior_item_id: prior_id.clone(),
            new_item,
            reason: "correct after erasure".into(),
            actor: "tester".into(),
            timestamp: erased_at.into(),
        })
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::ErasedSubject, "{err}");

    let prior = store
        .get_memory_item(&bank, &prior_id)
        .unwrap()
        .expect("prior remains after refused correction");
    assert!(prior.tx_until.is_none(), "prior must remain open");
    assert!(store.get_memory_item(&bank, &new_id).unwrap().is_none());

    store
        .test_execute(&format!(
            "DELETE FROM index_pending WHERE bank_id = '{bank}'; \
             DELETE FROM items WHERE bank_id = '{bank}'; \
             DELETE FROM subject_keys WHERE subject_id = '{subject}';"
        ))
        .unwrap();
}

const START: &str = "2026-06-01T00:00:00Z";
const WIDENED: &str = "2026-06-01T00:00:01Z";
const LATER: &str = "2026-07-01T00:00:00Z";

fn setup() -> Option<(crate::PgTestGuard, PostgresStore)> {
    let guard = lock_pg_test();
    let url = postgres_url_from_env();
    match PostgresStore::open(url.as_str(), local_kms()) {
        Ok(store) => Some((guard, store)),
        Err(err) => {
            crate::pg_skip::unreachable_pg_or_skip("postgres correction close stamps", &err);
            None
        }
    }
}

/// Run one correction of `prior_id` at `stamp` and leave its prior row closed.
fn correct(s: &PostgresStore, bank: &str, prior_id: &str, new_id: &str, stamp: &str) {
    let prior = s.get_memory_item(bank, prior_id).unwrap().unwrap();
    let mut new_item = prior.clone();
    new_item.id = new_id.to_owned();
    s.correct_item(&CorrectionSpec {
        bank_id: bank.to_owned(),
        prior_item_id: prior_id.to_owned(),
        new_item,
        reason: "same-second correction".to_owned(),
        actor: "tester".to_owned(),
        timestamp: stamp.to_owned(),
    })
    .unwrap();
}

#[test]
fn pg_correct_in_the_priors_starting_second_stores_one_second_later() {
    let Some((_guard, s)) = setup() else {
        return;
    };
    let bank = unique("b-pg-correct-same-second");
    let subject = unique("correct-same-subject");
    let prior_id = unique("item-prior");
    let new_id = unique("item-corrected");
    let prior = memory_item(&bank, &prior_id, &subject);
    assert_eq!(prior.tx_from, START, "fixture starts the row at {START}");
    s.create_memory_item(&prior, "tester").expect("create");

    correct(&s, &bank, &prior_id, &new_id, START);

    let row = s.get_memory_item(&bank, &prior_id).unwrap().unwrap();
    assert_eq!(
        row.tx_until.as_deref(),
        Some(WIDENED),
        "the close lands one second past the row's own start"
    );
    assert!(
        row.tx_from.as_str() < row.tx_until.as_deref().unwrap(),
        "a transaction read at {} sits inside [{:?})",
        row.tx_from,
        row.tx_until
    );
    assert_eq!(
        row.valid_until.as_deref(),
        Some(START),
        "the valid axis keeps the requested stamp"
    );
}

#[test]
fn pg_correct_after_the_priors_start_keeps_the_requested_stamp() {
    let Some((_guard, s)) = setup() else {
        return;
    };
    let bank = unique("b-pg-correct-later");
    let subject = unique("correct-later-subject");
    let prior_id = unique("item-prior");
    let new_id = unique("item-corrected");
    let prior = memory_item(&bank, &prior_id, &subject);
    s.create_memory_item(&prior, "tester").expect("create");

    correct(&s, &bank, &prior_id, &new_id, LATER);

    let row = s.get_memory_item(&bank, &prior_id).unwrap().unwrap();
    assert_eq!(
        row.tx_until.as_deref(),
        Some(LATER),
        "a later close is stored byte for byte"
    );
    assert_eq!(row.valid_until.as_deref(), Some(LATER));
}
