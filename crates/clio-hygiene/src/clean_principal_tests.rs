// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module pins what the presenting credential does to a durable hygiene
//! log row: a bound clean names the credential, a local clean names nobody, and
//! the label is recorded as text with nothing derived from it.
//!
//! ## Owns
//! - Test coverage for the credential key on a `hygiene_audit_logs` row
//! - Test coverage for its absence on an unbound row, dry-run rows included
//!
//! ## Does not own
//! - Clean counts, outcomes and validation (`clean_tests`)
//! - The log-row writer itself (`clean.rs`)
//!
//! ## Boundary
//!
//! Tests here break when the durable log's attribution changes; they never
//! change it.
//!
//! Keep this module focused on the credential label on a hygiene log row.

use super::clean_test_support::{args, bound_ctx, ctx, item, store, target};
use crate::clean::{CleanAction, clean};
use clio_store::{HygieneStore, Store};
use clio_types::{OpContext, PRINCIPAL_DETAIL_KEY, PrincipalLabel};

/// One confirmed clean, and the durable row it left, read back.
fn clean_and_read_log(bank: &str, id: &str, opctx: &OpContext) -> clio_store::HygieneLogEntry {
    let s = store();
    let s_ref = &*s as &dyn Store;
    s_ref
        .create_memory_item(&item(id, bank, "log me"), "actor")
        .unwrap();
    clean(&args(&s, opctx, &[target(id)], CleanAction::Archive, true)).unwrap();
    let rows = s.list_hygiene_logs(Some(bank), 50, 0).unwrap();
    assert_eq!(rows.len(), 1, "one log row per confirmed clean");
    rows.into_iter().next().expect("log row")
}

#[test]
fn a_bound_clean_names_the_credential_beside_the_claimed_actor() {
    let row = clean_and_read_log("bank-bound", "itm-b", &bound_ctx("bank-bound", "ops-key"));
    assert_eq!(row.actor, "agent", "the claimed role is unchanged");
    assert_eq!(row.detail[PRINCIPAL_DETAIL_KEY], "ops-key");
    assert_eq!(row.detail["result"], "applied");
    assert!(row.confirmed && !row.dry_run);
}

#[test]
fn a_local_clean_row_carries_no_credential_key() {
    let row = clean_and_read_log("bank-local", "itm-l", &ctx("bank-local"));
    assert_eq!(row.actor, "agent");
    assert!(
        row.detail.get(PRINCIPAL_DETAIL_KEY).is_none(),
        "an unbound operation must not carry an empty or null label: {}",
        row.detail
    );
}

#[test]
fn a_dry_run_row_is_labelled_the_same_way_as_a_confirmed_one() {
    let bank = "bank-dry";
    let s = store();
    let s_ref = &*s as &dyn Store;
    s_ref
        .create_memory_item(&item("itm-d", bank, "dry"), "actor")
        .unwrap();
    clean(&args(
        &s,
        &bound_ctx(bank, "ops-key"),
        &[target("itm-d")],
        CleanAction::Archive,
        false,
    ))
    .unwrap();
    let rows = s.list_hygiene_logs(Some(bank), 50, 0).unwrap();
    assert_eq!(rows.len(), 1);
    assert!(rows[0].dry_run, "an unconfirmed clean still writes a row");
    assert_eq!(rows[0].detail["result"], "would_apply");
    assert_eq!(rows[0].detail[PRINCIPAL_DETAIL_KEY], "ops-key");
}

#[test]
fn the_recorded_label_is_the_label_and_nothing_derived_from_a_secret() {
    let bank = "bank-label";
    let s = store();
    let s_ref = &*s as &dyn Store;
    s_ref
        .create_memory_item(&item("itm-s", bank, "secret"), "actor")
        .unwrap();
    clean(&args(
        &s,
        &bound_ctx(bank, "ops-key"),
        &[target("itm-s")],
        CleanAction::Archive,
        true,
    ))
    .unwrap();
    let rows = s.list_hygiene_logs(Some(bank), 50, 0).unwrap();
    let detail = rows[0].detail.to_string();
    assert_eq!(rows[0].detail[PRINCIPAL_DETAIL_KEY], "ops-key");
    assert!(
        !detail.contains("sha256") && !detail.contains("digest"),
        "the audited row names the credential and derives nothing: {detail}"
    );
    assert!(
        !detail.contains("secret"),
        "content never reaches a log row: {detail}"
    );
}

#[test]
fn the_label_survives_the_round_trip_through_the_store() {
    let bank = "bank-round";
    let s = store();
    let s_ref = &*s as &dyn Store;
    s_ref
        .create_memory_item(&item("itm-r", bank, "round"), "actor")
        .unwrap();
    clean(&args(
        &s,
        &bound_ctx(bank, "ops-key"),
        &[target("itm-r")],
        CleanAction::Archive,
        true,
    ))
    .unwrap();
    // A bank-wide listing returns the same row, so the key is stored, not added
    // on the way out.
    let row = s
        .list_hygiene_logs(None, 50, 0)
        .unwrap()
        .into_iter()
        .find(|r| r.item_id == "itm-r")
        .expect("row in the bank-wide listing");
    assert_eq!(row.detail[PRINCIPAL_DETAIL_KEY], "ops-key");
}

#[test]
fn two_credentials_on_the_same_bank_are_distinguishable() {
    let bank = "bank-two";
    let s = store();
    let s_ref = &*s as &dyn Store;
    s_ref
        .create_memory_item(&item("itm-1", bank, "one"), "actor")
        .unwrap();
    s_ref
        .create_memory_item(&item("itm-2", bank, "two"), "actor")
        .unwrap();
    for (id, label) in [("itm-1", "ops-key"), ("itm-2", "review-key")] {
        clean(&args(
            &s,
            &bound_ctx(bank, label),
            &[target(id)],
            CleanAction::Archive,
            true,
        ))
        .unwrap();
    }
    let rows = s.list_hygiene_logs(Some(bank), 50, 0).unwrap();
    assert_eq!(rows.len(), 2);
    for row in &rows {
        let label: &str = row.detail[PRINCIPAL_DETAIL_KEY].as_str().expect("a label");
        assert!(
            ["ops-key", "review-key"].contains(&label),
            "each row names the credential that acted: {label}"
        );
        assert_eq!(row.actor, "agent", "both claim the same role");
    }
    // The two labels are not interchangeable: they are what tells the rows apart.
    let mut labels: Vec<&str> = rows
        .iter()
        .map(|r| r.detail[PRINCIPAL_DETAIL_KEY].as_str().unwrap())
        .collect();
    labels.sort_unstable();
    assert_eq!(labels, vec!["ops-key", "review-key"]);
    let _ = PrincipalLabel::new("ops-key");
}
