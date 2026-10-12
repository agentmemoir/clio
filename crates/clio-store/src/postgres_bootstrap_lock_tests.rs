// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests that a Postgres store open serializes its bootstrap DDL
//! behind one cross-session advisory lock, so two overlapping opens do not
//! deadlock on the table locks that DDL takes.
//!
//! ## Owns
//! - The bootstrap-lock regression: a concurrent open waits for the lock
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

use super::{PG_BOOTSTRAP_ADVISORY_KEY, PostgresStore};
use crate::db_url::postgres_url_from_env;
use crate::erase_test_support::local_kms;
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
/// holder and the server can abort it with `deadlock detected`. The one-second
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

    assert!(
        rx.recv_timeout(Duration::from_secs(1)).is_err(),
        "open() ran its bootstrap while another session held the lock"
    );
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
/// the bootstrap lock the overlapping opens deadlock on table locks, and one of
/// them aborts before its command runs.
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
            std::thread::spawn(move || PostgresStore::open(&url, local_kms()).is_ok())
        })
        .collect();
    for opener in openers {
        assert!(
            opener.join().expect("join opener"),
            "an overlapping open failed"
        );
    }
}
