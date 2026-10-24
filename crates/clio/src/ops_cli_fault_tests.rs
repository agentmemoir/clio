// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `verify` and `doctor` failure exit paths of the
//! `clio ops` CLI: a planted integrity fault must report `ok:false` and exit 1.
//!
//! ## Owns
//! - The failing-verify and failing-doctor CLI paths over a temp-file SQLite store
//!
//! ## Does not own
//! - The verify checks themselves (`clio-ops`)
//! - Other `clio ops` exit-code tests (`ops_cli_tests.rs`)
//!
//! ## Boundary
//!
//! Tests here break when the verify exit contract changes; they never change it.
//!
//! Keep this module focused on the failing-verify exit path.

use super::*;
use clio_store::{AssocEdgeRecord, AssocKind, DekProvider, LocalDevKms, SqliteStore};
use std::sync::Arc;

#[test]
fn verify_with_planted_fault_exits_1() {
    let scratch = crate::backup_cli_fixtures::Scratch::new("ops-verify-fault");
    let db = scratch.join("clio.db");
    let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
    let store = SqliteStore::open(&db, Arc::clone(&kms)).expect("temp store");
    store
        .put_assoc_edge(&AssocEdgeRecord {
            id: "ghost-edge".into(),
            bank_id: "default".into(),
            source_id: "missing-a".into(),
            target_id: "missing-b".into(),
            assoc_kind: AssocKind::Explicit,
            relationship: Some("r".into()),
            weight: 0.5,
            last_updated_at: "t".into(),
        })
        .expect("plant orphan edge");
    drop(store);
    let env = |_key: &str| None;
    let args = [
        "verify",
        "--backend",
        "sqlite",
        "--db",
        &db.to_string_lossy(),
    ]
    .map(str::to_owned);
    assert_eq!(ops_with(args, &env), ExitCode::from(1));
    let doctor_args = [
        "doctor",
        "--backend",
        "sqlite",
        "--db",
        &db.to_string_lossy(),
    ]
    .map(str::to_owned);
    assert_eq!(ops_with(doctor_args, &env), ExitCode::from(1));
}

/// The fault sits in another bank, so a pass scoped to a clean bank finds nothing
/// of its own: it must still exit 1, exactly as the whole-store pass does, while
/// a store with no fault at all exits 0 for the same scoped arguments.
#[test]
fn verify_bank_scoped_with_an_outside_fault_exits_1() {
    let scratch = crate::backup_cli_fixtures::Scratch::new("ops-verify-bank-fault");
    let db = scratch.join("clio.db");
    let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
    let store = SqliteStore::open(&db, Arc::clone(&kms)).expect("temp store");
    for bank in ["default", "other"] {
        store.register_bank(bank, "t").expect("register bank");
    }
    store
        .put_assoc_edge(&AssocEdgeRecord {
            id: "ghost-edge".into(),
            bank_id: "other".into(),
            source_id: "missing-a".into(),
            target_id: "missing-b".into(),
            assoc_kind: AssocKind::Explicit,
            relationship: Some("r".into()),
            weight: 0.5,
            last_updated_at: "t".into(),
        })
        .expect("plant orphan edge");
    drop(store);

    let env = |_key: &str| None;
    let verify_args = |db: &std::path::Path, bank: Option<&str>| {
        let mut args: Vec<String> = ["verify", "--backend", "sqlite", "--db"]
            .iter()
            .map(|arg| (*arg).to_owned())
            .collect();
        args.push(db.to_string_lossy().into_owned());
        if let Some(bank) = bank {
            args.push("--bank".to_owned());
            args.push(bank.to_owned());
        }
        args
    };

    assert_eq!(
        ops_with(verify_args(&db, Some("default")), &env),
        ExitCode::from(1)
    );
    assert_eq!(ops_with(verify_args(&db, None), &env), ExitCode::from(1));

    let clean = scratch.join("clean.db");
    let store = SqliteStore::open(&clean, Arc::clone(&kms)).expect("clean store");
    store.register_bank("default", "t").expect("register bank");
    drop(store);
    assert_eq!(
        ops_with(verify_args(&clean, Some("default")), &env),
        ExitCode::SUCCESS
    );
}
