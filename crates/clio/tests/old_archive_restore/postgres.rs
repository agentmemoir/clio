// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the Postgres half of the old-archive restore
//! measurement: it archives before a synthetic erasure, erases one subject,
//! and reads the denied and control subjects back through each supported
//! restore route.
//!
//! ## Owns
//! - The Postgres per-route matrix and its observation log
//! - The fresh-fixture repeat and the control-only comparison for Postgres
//!
//! ## Does not own
//! - The archive assembly and read-back probes (see `support`)
//! - The SQLite half (see `sqlite`) or the post-erasure members (see `members`)
//!
//! ## Boundary
//!
//! Changes to which Postgres restore routes are measured should normally
//! require changes here. Changes to a restore primitive should not.
//!
//! Keep this module focused on the Postgres routes.

use crate::support::{
    Archive, CONTROL_ITEM, CONTROL_SUBJECT, DENIED_ITEM, DENIED_SUBJECT, Readback, Scratch,
    archive_postgres, erase_subject, observe, open_pg, pg_url, postgres_reachable, readback,
    subject_key_destroyed, write_item,
};
use crate::support_pg::require_postgres;
use clio::store::{ScopedSchema, lock_pg_test};
use std::path::Path;

/// What one Postgres run observed, per route.
struct Matrix {
    archive: Archive,
    live_denied: Readback,
    live_control: Readback,
    load_and_key_denied: Readback,
    load_and_key_destroyed: bool,
    load_only_denied: Readback,
    load_only_destroyed: bool,
    key_only_denied: Readback,
}

/// Copy a file, creating the destination's directory.
fn copy(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst.parent().expect("destination parent")).expect("create copy dir");
    std::fs::copy(src, dst).expect("copy the file");
}

/// Read the denied and control items and the denied key state from one schema.
fn reads(url: &str, kms: &Path) -> (Readback, Readback, bool) {
    let store = open_pg(url, kms);
    (
        readback(&store, DENIED_ITEM),
        readback(&store, CONTROL_ITEM),
        subject_key_destroyed(&store, DENIED_SUBJECT),
    )
}

/// Create a schema's tables by opening and dropping a store over it.
fn migrate(url: &str, kms: &Path) {
    drop(open_pg(url, kms));
}

/// Run the Postgres archive-then-erase-then-restore matrix once.
fn run_matrix(tag: &str, base: &str) -> Matrix {
    let scratch = Scratch::new(tag);
    let a = ScopedSchema::new(base);
    let a_kms = scratch.join("a/clio.kms.json");

    // Seed the deployment, then archive it before any erasure.
    let store = open_pg(a.url(), &a_kms);
    write_item(&store, DENIED_ITEM, DENIED_SUBJECT, "denied sentinel gist");
    write_item(
        &store,
        CONTROL_ITEM,
        CONTROL_SUBJECT,
        "control sentinel gist",
    );
    drop(store);
    let archive = archive_postgres(&scratch.join("a/backups"), a.url(), &a_kms, "pg-old");

    // Erase the denied subject in the live deployment and measure it.
    let (live_denied, live_control, live_destroyed) = {
        let store = open_pg(a.url(), &a_kms);
        erase_subject(&store, DENIED_SUBJECT, "req-1");
        (
            readback(&store, DENIED_ITEM),
            readback(&store, CONTROL_ITEM),
            subject_key_destroyed(&store, DENIED_SUBJECT),
        )
    };
    observe(
        "postgres",
        "live-post-erase",
        &archive,
        &live_denied,
        live_destroyed,
    );

    // Route 1: load the archived rows and place the archived key member.
    let b = ScopedSchema::new(base);
    let b_kms = scratch.join("b/clio.kms.json");
    migrate(b.url(), &b_kms);
    crate::support::restore_pg(&archive, &b, &b_kms, &scratch.join("b/extract"), true);
    let (load_and_key_denied, _, load_and_key_destroyed) = reads(b.url(), &b_kms);
    observe(
        "postgres",
        "load+key",
        &archive,
        &load_and_key_denied,
        load_and_key_destroyed,
    );

    // Route 2: load the archived rows, keep the live post-erasure key store.
    let c = ScopedSchema::new(base);
    let c_kms = scratch.join("c/clio.kms.json");
    migrate(c.url(), &c_kms);
    copy(&a_kms, &c_kms);
    crate::support::restore_pg(&archive, &c, &c_kms, &scratch.join("c/extract"), false);
    let (load_only_denied, _, load_only_destroyed) = reads(c.url(), &c_kms);
    observe(
        "postgres",
        "load-only",
        &archive,
        &load_only_denied,
        load_only_destroyed,
    );

    // Route 3: place only the archived key member, over the post-erasure rows.
    let _ = crate::support::restore_key_only(&archive, &a_kms);
    let (key_only_denied, _, key_only_destroyed) = reads(a.url(), &a_kms);
    observe(
        "postgres",
        "key-only",
        &archive,
        &key_only_denied,
        key_only_destroyed,
    );

    Matrix {
        archive,
        live_denied,
        live_control,
        load_and_key_denied,
        load_and_key_destroyed,
        load_only_denied,
        load_only_destroyed,
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
        matrix.load_and_key_denied,
        Readback::Readable,
        "a pre-erasure archive's rows and key member restore the denied content"
    );
    assert!(
        !matrix.load_and_key_destroyed,
        "the archived key member carries the still-active wrapping material"
    );
    assert_eq!(
        matrix.load_only_denied,
        Readback::DestroyedKey,
        "the rows alone, over the post-erasure key store, stay unreadable"
    );
    assert!(
        matrix.load_only_destroyed,
        "the post-erasure key store keeps the destroyed slot"
    );
    assert_eq!(
        matrix.key_only_denied,
        Readback::Readable,
        "the archived key member alone re-opens the post-erasure rows"
    );
}

#[test]
fn postgres_pre_erasure_archive_re_exposes_the_denied_subject() {
    let base = pg_url();
    if !require_postgres(postgres_reachable(&base)) {
        return;
    }
    let _guard = lock_pg_test();
    let matrix = run_matrix("pg-t01", &base);
    assert_matrix(&matrix);
    println!(
        "MEASURED postgres archive={} keys_included={}",
        matrix.archive.id, matrix.archive.manifest.keys.included
    );
}

#[test]
fn postgres_fresh_fixture_repeat_matches_the_first_run() {
    let base = pg_url();
    if !require_postgres(postgres_reachable(&base)) {
        return;
    }
    let _guard = lock_pg_test();
    let matrix = run_matrix("pg-t03", &base);
    assert_matrix(&matrix);
    println!(
        "MEASURED postgres-repeat archive={} keys_included={}",
        matrix.archive.id, matrix.archive.manifest.keys.included
    );
}

#[test]
fn postgres_control_only_archive_restores_only_the_control_subject() {
    let base = pg_url();
    if !require_postgres(postgres_reachable(&base)) {
        return;
    }
    let _guard = lock_pg_test();
    let scratch = Scratch::new("pg-t04");

    // The refusal input: a deployment that held the denied subject.
    let p = ScopedSchema::new(&base);
    let p_kms = scratch.join("p/clio.kms.json");
    let store = open_pg(p.url(), &p_kms);
    write_item(&store, DENIED_ITEM, DENIED_SUBJECT, "denied sentinel gist");
    write_item(
        &store,
        CONTROL_ITEM,
        CONTROL_SUBJECT,
        "control sentinel gist",
    );
    drop(store);
    let refusal = archive_postgres(&scratch.join("p/backups"), p.url(), &p_kms, "pg-refusal");

    // The positive control: a deployment where the denied subject never existed.
    let ctl = ScopedSchema::new(&base);
    let ctl_kms = scratch.join("ctl/clio.kms.json");
    let store = open_pg(ctl.url(), &ctl_kms);
    write_item(
        &store,
        CONTROL_ITEM,
        CONTROL_SUBJECT,
        "control sentinel gist",
    );
    drop(store);
    let control = archive_postgres(
        &scratch.join("ctl/backups"),
        ctl.url(),
        &ctl_kms,
        "pg-control",
    );

    // Restoring the refusal input re-exposes the denied subject.
    let r = ScopedSchema::new(&base);
    let r_kms = scratch.join("r/clio.kms.json");
    migrate(r.url(), &r_kms);
    crate::support::restore_pg(&refusal, &r, &r_kms, &scratch.join("r/extract"), true);
    let refusal_denied = readback(&open_pg(r.url(), &r_kms), DENIED_ITEM);
    observe(
        "postgres",
        "refusal-input",
        &refusal,
        &refusal_denied,
        false,
    );

    // Restoring the positive control never introduces the denied subject.
    let o = ScopedSchema::new(&base);
    let o_kms = scratch.join("o/clio.kms.json");
    migrate(o.url(), &o_kms);
    crate::support::restore_pg(&control, &o, &o_kms, &scratch.join("o/extract"), true);
    let o_store = open_pg(o.url(), &o_kms);
    let control_denied = readback(&o_store, DENIED_ITEM);
    let control_control = readback(&o_store, CONTROL_ITEM);
    observe(
        "postgres",
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
        "INPUTS postgres refusal={} positive_control={}",
        refusal.id, control.id
    );
}
