// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the Postgres distilled-unit revision close
//! stamps owned by `postgres_distilled_unit`: a prior revision closed in the
//! second it started keeps a transaction interval that is not empty.
//!
//! ## Owns
//! - The same-second revision case and the later-stamp guard on that path
//!
//! ## Does not own
//! - The rest of the Postgres revision contract (`postgres_distilled_unit_tests`)
//! - The close-stamp rule itself (`migrate_canonical_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds. Changes to what a
//! same-second revision leaves stored on the prior row should normally update
//! tests here.

use crate::PgTestGuard;
use crate::db_url::postgres_url_or_default;
use crate::dek::{DekProvider, LocalDevKms};
use crate::distilled_unit::distilled_unit_reconcile_tests::record;
use crate::distilled_unit::distilled_unit_tests::hub_item;
use crate::distilled_unit_store::DistilledUnitStore;
use crate::pg_lock::lock_pg_test;
use crate::postgres::PostgresStore;
use crate::store::Store;
use crate::test_unique::unique;
use std::sync::Arc;

const START: &str = "2026-06-01T00:00:00Z";
const WIDENED: &str = "2026-06-01T00:00:01Z";

fn setup() -> Option<(PgTestGuard, PostgresStore)> {
    let guard = lock_pg_test();
    let url = postgres_url_or_default(std::env::var("CLIO_DATABASE_URL"));
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    match PostgresStore::open(&url, kms) {
        Ok(store) => Some((guard, store)),
        Err(err) => {
            crate::pg_skip::unreachable_pg_or_skip("postgres distilled revision stamps", &err);
            None
        }
    }
}

#[test]
fn pg_revise_in_the_priors_starting_second_stores_one_second_later() {
    let Some((_guard, store)) = setup() else {
        return;
    };
    let bank = unique("b-pg-revise-same-second");
    let subject = unique("subject");
    let hub = unique("hub");
    let a = unique("source-a");
    let item_id = format!("hub-distill:{bank}:{hub}");
    let first = hub_item(&bank, &item_id, &subject, &hub, &[&a]);
    let first_record = record(&bank, &item_id, &hub, std::slice::from_ref(&a));
    store
        .commit_distilled_unit(&first, &first_record, "tester")
        .unwrap();
    assert_eq!(first.tx_from, START);

    let mut second_record = record(&bank, &item_id, &hub, std::slice::from_ref(&a));
    second_record.revision = 2;
    second_record.distilled_at = START.to_owned();
    let second = hub_item(
        &bank,
        &second_record.current_revision_item_id(),
        &subject,
        &hub,
        &[&a],
    );
    store
        .revise_distilled_unit(&second_record, &second, "tester")
        .unwrap();

    let prior = store.get_memory_item(&bank, &item_id).unwrap().unwrap();
    assert_eq!(
        prior.tx_until.as_deref(),
        Some(WIDENED),
        "the close lands one second past the prior's own start"
    );
    assert!(
        prior.tx_from.as_str() < prior.tx_until.as_deref().unwrap(),
        "a transaction read at {} sits inside [{:?})",
        prior.tx_from,
        prior.tx_until
    );
    assert_eq!(
        prior.valid_until.as_deref(),
        Some(START),
        "the valid axis keeps the distilled stamp"
    );
}

#[test]
fn pg_revise_after_the_priors_start_keeps_the_distilled_stamp() {
    let Some((_guard, store)) = setup() else {
        return;
    };
    let bank = unique("b-pg-revise-later");
    let subject = unique("subject");
    let hub = unique("hub");
    let a = unique("source-a");
    let item_id = format!("hub-distill:{bank}:{hub}");
    let first = hub_item(&bank, &item_id, &subject, &hub, &[&a]);
    let first_record = record(&bank, &item_id, &hub, std::slice::from_ref(&a));
    store
        .commit_distilled_unit(&first, &first_record, "tester")
        .unwrap();

    let mut second_record = record(&bank, &item_id, &hub, std::slice::from_ref(&a));
    second_record.revision = 2;
    second_record.distilled_at = "2026-07-01T00:00:00Z".to_owned();
    let second = hub_item(
        &bank,
        &second_record.current_revision_item_id(),
        &subject,
        &hub,
        &[&a],
    );
    store
        .revise_distilled_unit(&second_record, &second, "tester")
        .unwrap();

    let prior = store.get_memory_item(&bank, &item_id).unwrap().unwrap();
    assert_eq!(
        prior.tx_until.as_deref(),
        Some("2026-07-01T00:00:00Z"),
        "a later close is stored byte for byte"
    );
    assert_eq!(prior.valid_until.as_deref(), Some("2026-07-01T00:00:00Z"));
}
