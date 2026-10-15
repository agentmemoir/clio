// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the negative half of the old-archive restore
//! measurement: it proves the compliance erasure call refuses an unauthorized
//! actor and an unconfirmed call, leaving the subject's item readable and its
//! key intact.
//!
//! ## Owns
//! - The unauthorized-erase refusal and its no-write observation
//! - The unconfirmed-erase refusal and its no-key-destruction observation
//!
//! ## Does not own
//! - The synthetic fixtures and read-back probes (see `support`)
//! - The erasure workflow itself (see `clio-compliance`)
//!
//! ## Boundary
//!
//! Changes to which erasure calls are measured for refusal should normally
//! require changes here. Changes to the fixtures should not.
//!
//! Keep this module focused on the erasure authorization negatives.

use crate::support::{
    BANK, DENIED_ITEM, DENIED_SUBJECT, Readback, Scratch, journal_path, open_sqlite, readback,
    subject_key_destroyed, write_item,
};
use clio::{Actor, Bank, ErrorCode, OpContext};
use clio_compliance::{EraseArgs, EraseVecSink, erase_request, tombstone_salt};

#[test]
fn unauthorized_and_unconfirmed_erases_are_refused_without_state_change() {
    let scratch = Scratch::new("negative-t01");
    let db = scratch.join("a/clio.db");
    let kms = scratch.join("a/clio.kms.json");
    std::fs::create_dir_all(db.parent().expect("db parent")).expect("create a");
    let store = open_sqlite(&db, &kms);
    write_item(&store, DENIED_ITEM, DENIED_SUBJECT, "denied sentinel gist");

    let sink = EraseVecSink::default();
    let salt = tombstone_salt();
    // A sibling of the scenario's scratch root; both refusals below precede
    // any journal write, so nothing is recorded at this path.
    let journal = journal_path("negative-journal");

    // A user actor carries no compliance erasure privilege.
    let user = OpContext::new(Bank::new(BANK), Actor::User);
    let refused = erase_request(&EraseArgs {
        store: &store,
        journal_path: &journal,
        maint: None,
        ctx: &user,
        subject_id: DENIED_SUBJECT,
        legal_basis: "GDPR_ART_17",
        request_id: "req-negative-user",
        reason_code: None,
        confirm: true,
        salt: &salt,
        timestamp: "2026-09-17T11:00:00Z",
        sink: &sink,
    });
    let err = refused.expect_err("a user actor must not run an erasure");
    assert_eq!(
        err.code,
        ErrorCode::Forbidden,
        "the refusal names the missing privilege"
    );
    assert_eq!(
        readback(&store, DENIED_ITEM),
        Readback::Readable,
        "an unauthorized erasure must leave the subject's item readable"
    );
    assert!(
        !subject_key_destroyed(&store, DENIED_SUBJECT),
        "an unauthorized erasure must not destroy the subject's key"
    );

    // A privileged actor without confirmation never mutates.
    let harness = OpContext::new(Bank::new(BANK), Actor::Harness);
    let unconfirmed = erase_request(&EraseArgs {
        store: &store,
        journal_path: &journal,
        maint: None,
        ctx: &harness,
        subject_id: DENIED_SUBJECT,
        legal_basis: "GDPR_ART_17",
        request_id: "req-negative-unconfirmed",
        reason_code: None,
        confirm: false,
        salt: &salt,
        timestamp: "2026-09-17T11:00:00Z",
        sink: &sink,
    });
    let confirm_err = unconfirmed.expect_err("an unconfirmed erasure must be refused");
    assert_eq!(
        confirm_err.code,
        ErrorCode::Forbidden,
        "the refusal names the missing confirmation"
    );
    assert_eq!(
        readback(&store, DENIED_ITEM),
        Readback::Readable,
        "an unconfirmed erasure must leave the subject's item readable"
    );
    assert!(
        !subject_key_destroyed(&store, DENIED_SUBJECT),
        "an unconfirmed erasure must not destroy the subject's key"
    );

    println!(
        "REFUSED backend=sqlite unauthorized={} unconfirmed={} key_destroyed={}",
        err.code.as_str(),
        confirm_err.code.as_str(),
        subject_key_destroyed(&store, DENIED_SUBJECT)
    );
}
