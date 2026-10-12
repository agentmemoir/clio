// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests that a Postgres store open serializes its bootstrap DDL
//! behind one cross-session advisory lock, so two overlapping opens do not
//! fail on the table locks or the catalog rows that DDL takes.
//!
//! ## Owns
//! - The bootstrap-lock regression: a concurrent open waits for the lock
//! - The release guarantee on the refused-bootstrap path
//!
//! ## Does not own
//! - Shared-Postgres test serialization (`pg_lock`)
//! - Bootstrap order or DDL text (`postgres`)
//!
//! ## Boundary
//!
//! Tests here break when `PostgresStore::open` stops taking the bootstrap lock.
//!
//! Test-only. Never compiled into production builds.

use super::{PG_BOOTSTRAP_ADVISORY_KEY, PostgresStore, with_bootstrap_lock};
use crate::db_url::postgres_url_from_env;
use crate::erase_test_support::local_kms;
use clio_types::{AmError, ErrorCode};
use std::sync::mpsc;
use std::time::Duration;

/// The shared Postgres URL, or `None` when the database is unreachable.
fn shared_url() -> Option<String> {
    let url = postgres_url_from_env();
    crate::pg_tls::connect(&url).ok().map(|_| url)
}

/// An open waits while another session holds the bootstrap lock, then finishes
/// once the lock is released.
///
/// The wait is the point: without the lock the open runs its DDL beside the
/// holder and the server can abort it with `deadlock detected`,
/// `tuple concurrently updated`, or a `pg_type` unique-constraint violation. The one-second
/// window is a bound, not synchronization — an open that returned that fast
/// would prove it never took the lock. The release that follows unblocks the
/// open that did take it.
#[test]
fn an_open_waits_for_the_bootstrap_lock() {
    let _shared = crate::lock_pg_test();
    let Some(url) = shared_url() else {
        eprintln!("skip: shared Postgres unreachable");
        return;
    };
    let mut probe = crate::pg_tls::connect(&url).expect("probe connect");
    probe
        .query_one("SELECT pg_advisory_lock($1)", &[&PG_BOOTSTRAP_ADVISORY_KEY])
        .expect("hold the bootstrap lock");

    let (tx, rx) = mpsc::channel();
    let opener = std::thread::spawn(move || {
        let opened = PostgresStore::open(&url, local_kms()).is_ok();
        let _ = tx.send(opened);
    });

    match rx.recv_timeout(Duration::from_secs(1)) {
        Err(mpsc::RecvTimeoutError::Timeout) => {}
        other => panic!("open() did not wait for the bootstrap lock: {other:?}"),
    }
    drop(probe);
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(60)),
        Ok(true),
        "open() must succeed once the bootstrap lock is released"
    );
    opener.join().expect("join opener");
}

/// Four opens that overlap all finish.
///
/// This mirrors the reported reproduction of two concurrent commands: without
/// the bootstrap lock the overlapping opens race on table locks and catalog
/// rows, and one of them aborts with `deadlock detected`,
/// `tuple concurrently updated`, or a `pg_type` unique-constraint violation
/// before its command runs. The failure message is kept in the assertion so
/// a regression names which race returned.
#[test]
fn overlapping_opens_all_finish() {
    let _shared = crate::lock_pg_test();
    let Some(url) = shared_url() else {
        eprintln!("skip: shared Postgres unreachable");
        return;
    };
    let openers: Vec<_> = (0..4)
        .map(|_| {
            let url = url.clone();
            std::thread::spawn(move || {
                PostgresStore::open(&url, local_kms())
                    .map(|_| ())
                    .map_err(|error| error.to_string())
            })
        })
        .collect();
    for opener in openers {
        let opened = opener.join().expect("join opener");
        assert!(
            opened.is_ok(),
            "an overlapping open failed: {}",
            opened.err().unwrap_or_default()
        );
    }
}

/// A refused bootstrap must not leave the lock held.
///
/// A session advisory lock outlives a rolled-back transaction, so if the release
/// were skipped on the error path nothing would free the key before the
/// connection ended, and every later open would stall behind it. `open` runs its
/// bootstrap through this same helper, so this pins the contract it depends on.
#[test]
fn a_refused_bootstrap_releases_the_lock() {
    let _shared = crate::lock_pg_test();
    let Some(url) = shared_url() else {
        eprintln!("skip: shared Postgres unreachable");
        return;
    };
    let mut client = crate::pg_tls::connect(&url).expect("connect");
    let refused: Result<(), AmError> = with_bootstrap_lock(&mut client, |_| {
        Err(AmError::new(ErrorCode::Internal, "refused on purpose"))
    });
    assert!(refused.is_err(), "the body's error must reach the caller");

    let mut probe = crate::pg_tls::connect(&url).expect("probe connect");
    let free: bool = probe
        .query_one(
            "SELECT pg_try_advisory_lock($1)",
            &[&PG_BOOTSTRAP_ADVISORY_KEY],
        )
        .expect("probe try-lock")
        .get(0);
    assert!(free, "a refused bootstrap left the bootstrap lock held");
    probe
        .query_one(
            "SELECT pg_advisory_unlock($1)",
            &[&PG_BOOTSTRAP_ADVISORY_KEY],
        )
        .expect("release the probe lock");
}
