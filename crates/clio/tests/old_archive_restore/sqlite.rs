// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the SQLite half of the old-archive restore
//! measurement: it archives before a synthetic erasure, erases one subject,
//! and reads the denied and control subjects back through each supported
//! restore route.
//!
//! ## Owns
//! - The SQLite per-route matrix and its observation log
//! - The fresh-fixture repeat and the control-only comparison for SQLite
//!
//! ## Does not own
//! - The archive assembly and read-back probes (see `support`)
//! - The Postgres half (see `postgres`) or the post-erasure members (see
//!   `members`)
//!
//! ## Boundary
//!
//! Changes to which SQLite restore routes are measured should normally require
//! changes here. Changes to a restore primitive should not.
//!
//! Keep this module focused on the SQLite routes.

use crate::support::{
    Archive, CONTROL_ITEM, CONTROL_SUBJECT, DENIED_ITEM, DENIED_SUBJECT, Readback, Scratch,
    archive_sqlite, erase_subject, observe, open_sqlite, readback, subject_key_destroyed,
    write_item,
};
use std::path::Path;

/// What one SQLite run observed, per route.
struct Matrix {
    archive: Archive,
    live_denied: Readback,
    live_control: Readback,
    payload_and_key_denied: Readback,
    payload_and_key_destroyed: bool,
    payload_only_denied: Readback,
    payload_only_destroyed: bool,
    key_only_denied: Readback,
}

/// Copy a file, creating the destination's directory.
fn copy(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst.parent().expect("destination parent")).expect("create copy dir");
    std::fs::copy(src, dst).expect("copy the file");
}

/// Run the SQLite archive-then-erase-then-restore matrix once.
fn run_matrix(tag: &str) -> Matrix {
    let scratch = Scratch::new(tag);
    let a_db = scratch.join("a/clio.db");
    let a_kms = scratch.join("a/clio.kms.json");
    std::fs::create_dir_all(a_db.parent().expect("a parent")).expect("create a");

    // Seed the deployment, then archive it before any erasure.
    let store = open_sqlite(&a_db, &a_kms);
    write_item(&store, DENIED_ITEM, DENIED_SUBJECT, "denied sentinel gist");
    write_item(
        &store,
        CONTROL_ITEM,
        CONTROL_SUBJECT,
        "control sentinel gist",
    );
    drop(store);
    let archive = archive_sqlite(&scratch.join("a/backups"), &a_db, &a_kms, "sqlite-old");

    // Erase the denied subject in the live deployment and measure it.
    let (live_denied, live_control, live_destroyed) = {
        let store = open_sqlite(&a_db, &a_kms);
        erase_subject(&store, DENIED_SUBJECT, "req-1");
        let denied = readback(&store, DENIED_ITEM);
        let control = readback(&store, CONTROL_ITEM);
        (
            denied,
            control,
            subject_key_destroyed(&store, DENIED_SUBJECT),
        )
    };
    observe(
        "sqlite",
        "live-post-erase",
        &archive,
        &live_denied,
        live_destroyed,
    );

    // Route 1: place the archived payload and the archived key member.
    let b_db = scratch.join("b/clio.db");
    let b_kms = scratch.join("b/clio.kms.json");
    crate::support::restore_sqlite(&archive, &b_db, &b_kms, true);
    let (payload_and_key_denied, payload_and_key_destroyed) = {
        let store = open_sqlite(&b_db, &b_kms);
        (
            readback(&store, DENIED_ITEM),
            subject_key_destroyed(&store, DENIED_SUBJECT),
        )
    };
    observe(
        "sqlite",
        "payload+key",
        &archive,
        &payload_and_key_denied,
        payload_and_key_destroyed,
    );

    // Route 2: place the archived payload, keep the live post-erasure key store.
    let c_db = scratch.join("c/clio.db");
    let c_kms = scratch.join("c/clio.kms.json");
    copy(&a_kms, &c_kms);
    crate::support::restore_sqlite(&archive, &c_db, &c_kms, false);
    let (payload_only_denied, payload_only_destroyed) = {
        let store = open_sqlite(&c_db, &c_kms);
        (
            readback(&store, DENIED_ITEM),
            subject_key_destroyed(&store, DENIED_SUBJECT),
        )
    };
    observe(
        "sqlite",
        "payload-only",
        &archive,
        &payload_only_denied,
        payload_only_destroyed,
    );

    // Route 3: place only the archived key member, over the post-erasure payload.
    let d_db = scratch.join("d/clio.db");
    let d_kms = scratch.join("d/clio.kms.json");
    copy(&a_db, &d_db);
    copy(&a_kms, &d_kms);
    let _ = crate::support::restore_key_only(&archive, &d_kms);
    let key_only_denied = readback(&open_sqlite(&d_db, &d_kms), DENIED_ITEM);
    let key_only_destroyed = {
        let store = open_sqlite(&d_db, &d_kms);
        subject_key_destroyed(&store, DENIED_SUBJECT)
    };
    observe(
        "sqlite",
        "key-only",
        &archive,
        &key_only_denied,
        key_only_destroyed,
    );

    Matrix {
        archive,
        live_denied,
        live_control,
        payload_and_key_denied,
        payload_and_key_destroyed,
        payload_only_denied,
        payload_only_destroyed,
        key_only_denied,
    }
}

/// Assert one matrix's measured outcomes against what the routes did.
fn assert_matrix(matrix: &Matrix) {
    assert_eq!(
        matrix.live_denied,
        Readback::DestroyedKey,
        "the live post-erasure deployment must not decrypt the denied subject"
    );
    assert_eq!(
        matrix.live_control,
        Readback::Readable,
        "the control subject is untouched by the erasure"
    );
    assert_eq!(
        matrix.payload_and_key_denied,
        Readback::Readable,
        "a pre-erasure archive's payload and key member restore the denied content"
    );
    assert!(
        !matrix.payload_and_key_destroyed,
        "the archived key member carries the still-active wrapping material"
    );
    assert_eq!(
        matrix.payload_only_denied,
        Readback::DestroyedKey,
        "the payload alone, over the post-erasure key store, stays unreadable"
    );
    assert!(
        matrix.payload_only_destroyed,
        "the post-erasure key store keeps the destroyed slot"
    );
    assert_eq!(
        matrix.key_only_denied,
        Readback::Readable,
        "the archived key member alone re-opens the post-erasure payload"
    );
}

#[test]
fn sqlite_pre_erasure_archive_re_exposes_the_denied_subject() {
    let matrix = run_matrix("sqlite-t01");
    assert_matrix(&matrix);
    println!(
        "MEASURED sqlite archive={} keys_included={}",
        matrix.archive.id, matrix.archive.manifest.keys.included
    );
}

#[test]
fn sqlite_fresh_fixture_repeat_matches_the_first_run() {
    let matrix = run_matrix("sqlite-t03");
    assert_matrix(&matrix);
    println!(
        "MEASURED sqlite-repeat archive={} keys_included={}",
        matrix.archive.id, matrix.archive.manifest.keys.included
    );
}

#[test]
fn sqlite_control_only_archive_restores_only_the_control_subject() {
    let scratch = Scratch::new("sqlite-t04");
    // The refusal input: a deployment that held the denied subject, archived
    // before any erasure.
    let p_db = scratch.join("p/clio.db");
    let p_kms = scratch.join("p/clio.kms.json");
    std::fs::create_dir_all(p_db.parent().expect("p parent")).expect("create p");
    let store = open_sqlite(&p_db, &p_kms);
    write_item(&store, DENIED_ITEM, DENIED_SUBJECT, "denied sentinel gist");
    write_item(
        &store,
        CONTROL_ITEM,
        CONTROL_SUBJECT,
        "control sentinel gist",
    );
    drop(store);
    let refusal = archive_sqlite(&scratch.join("p/backups"), &p_db, &p_kms, "sqlite-refusal");

    // The positive control: a deployment where the denied subject never existed.
    let ctl_db = scratch.join("ctl/clio.db");
    let ctl_kms = scratch.join("ctl/clio.kms.json");
    std::fs::create_dir_all(ctl_db.parent().expect("ctl parent")).expect("create ctl");
    let store = open_sqlite(&ctl_db, &ctl_kms);
    write_item(
        &store,
        CONTROL_ITEM,
        CONTROL_SUBJECT,
        "control sentinel gist",
    );
    drop(store);
    let control = archive_sqlite(
        &scratch.join("ctl/backups"),
        &ctl_db,
        &ctl_kms,
        "sqlite-control",
    );

    // Restoring the refusal input re-exposes the denied subject.
    let r_db = scratch.join("r/clio.db");
    let r_kms = scratch.join("r/clio.kms.json");
    crate::support::restore_sqlite(&refusal, &r_db, &r_kms, true);
    let r_store = open_sqlite(&r_db, &r_kms);
    let refusal_denied = readback(&r_store, DENIED_ITEM);
    observe("sqlite", "refusal-input", &refusal, &refusal_denied, false);

    // Restoring the positive control never introduces the denied subject.
    let o_db = scratch.join("o/clio.db");
    let o_kms = scratch.join("o/clio.kms.json");
    crate::support::restore_sqlite(&control, &o_db, &o_kms, true);
    let o_store = open_sqlite(&o_db, &o_kms);
    let control_denied = readback(&o_store, DENIED_ITEM);
    let control_control = readback(&o_store, CONTROL_ITEM);
    observe(
        "sqlite",
        "positive-control",
        &control,
        &control_denied,
        false,
    );

    assert_eq!(
        refusal_denied,
        Readback::Readable,
        "the refusal input re-exposes the denied subject"
    );
    assert_eq!(
        control_denied,
        Readback::Absent,
        "the positive control never held the denied subject"
    );
    assert_eq!(
        control_control,
        Readback::Readable,
        "the positive control keeps the permitted subject readable"
    );
    println!(
        "INPUTS sqlite refusal={} positive_control={}",
        refusal.id, control.id
    );
}
