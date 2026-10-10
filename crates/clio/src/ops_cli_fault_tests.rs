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
