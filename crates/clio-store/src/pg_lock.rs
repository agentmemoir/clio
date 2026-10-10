// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for serializing tests that touch the shared
//! `CLIO_DATABASE_URL` / Compose Postgres instance: inside one test process and
//! across overlapping test processes.
//!
//! ## Owns
//! - [`PG_TEST_LOCK`], [`lock_pg_test`], and the advisory-lock half of the guard
//!
//! ## Does not own
//! - Postgres SQL or schema handling (`clio_store` backends)
//! - Private-schema isolation for store-global assertions (`pg_scoped_schema`)
//!
//! ## Boundary
//!
//! Changes to which tests need serialization should update their callers.
//! Changes to the lock mechanics should normally require changes here.
//!
//! Keep this module focused on shared-Postgres test serialization.
//!
//! # What the guard covers
//!
//! `cargo test` runs one test target at a time, and libtest runs the tests inside
//! a target on parallel threads, so the in-process mutex is what keeps two tests
//! of one binary off the shared database at once. A mutex cannot see another
//! process, so a best-effort Postgres advisory lock also covers the case where a
//! developer or CI overlaps two test runs against the same database: the later
//! run waits instead of interleaving. When the database is unreachable the guard
//! degrades to in-process serialization only, so a test that skips without
//! Postgres keeps skipping.

use crate::db_url::postgres_url_or_default;
use postgres::Client;
use std::sync::{Mutex, MutexGuard, PoisonError};

/// Advisory-lock key for the shared test database (`"clio_tst"` as hex).
const PG_TEST_ADVISORY_KEY: i64 = 0x636C_696F_5F74_7374;

/// Advisory-lock key used only by this module's own tests (`"clio_tst2"`).
///
/// The wait test must not contend for [`PG_TEST_ADVISORY_KEY`]: every
/// shared-Postgres test in the workspace takes that key, and `cargo test
/// --workspace` runs several test binaries at once, so a foreign holder could
/// take the key between the test's release and its waiter's acquire. The waiter
/// would then block behind a process that releases it seconds later, and the
/// assertion that it acquires promptly would fail at random. A private key makes
/// the only possible competitor this test itself.
#[cfg(test)]
const PG_TEST_PROBE_KEY: i64 = 0x636C_696F_5F74_7374 ^ 0x32;

/// Serialize tests that share `CLIO_DATABASE_URL` / Compose Postgres.
pub static PG_TEST_LOCK: Mutex<()> = Mutex::new(());

/// One shared-Postgres test's hold on the shared database.
///
/// Field order is the drop order: the advisory-lock session closes first
/// (releasing the cross-process lock), the in-process mutex second.
#[must_use = "the guard releases both locks when dropped; bind it for the test's duration"]
pub struct PgTestGuard {
    _advisory: Option<Client>,
    _process: MutexGuard<'static, ()>,
}

/// Acquire [`PG_TEST_LOCK`] (recovering from a poisoned one) and the
/// shared-database advisory lock, waiting when another test process holds it.
pub fn lock_pg_test() -> PgTestGuard {
    let process = PG_TEST_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    let url = postgres_url_or_default(clio_config::db_url_env::database_url_from_process_env());
    PgTestGuard {
        _advisory: advisory_lock(&url, PG_TEST_ADVISORY_KEY),
        _process: process,
    }
}

/// Take `key` in this session, blocking while another session holds it.
///
/// Returns `None` when the database is unreachable or the lock cannot be taken,
/// which leaves the caller with in-process serialization only — the state every
/// Postgres-optional test already tolerates.
fn advisory_lock(url: &str, key: i64) -> Option<Client> {
    let mut client = crate::pg_tls::connect(url).ok()?;
    let held: bool = client
        .query_one("SELECT pg_try_advisory_lock($1)", &[&key])
        .ok()?
        .get(0);
    if !held {
        eprintln!(
            "clio: waiting for the shared-Postgres test lock (another test process holds it)"
        );
        client
            .query_one("SELECT pg_advisory_lock($1)", &[&key])
            .ok()?;
    }
    Some(client)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn unreachable_database_degrades_to_in_process_only() {
        // Loopback port 1 refuses immediately: no shared database, no lock, and
        // the caller still gets a usable guard.
        assert!(
            advisory_lock(
                "postgres://clio:clio@127.0.0.1:1/clio",
                PG_TEST_ADVISORY_KEY
            )
            .is_none()
        );
        let _guard = lock_pg_test();
    }

    /// The wait property, on a key no other test contends for.
    ///
    /// `PG_TEST_PROBE_KEY` is private to this module, so the only session that
    /// can delay the waiter is the holder created here. That is what makes the
    /// "acquires promptly after release" half deterministic under
    /// `cargo test --workspace`, which runs several binaries in parallel.
    #[test]
    fn a_second_holder_waits_until_the_first_releases() {
        let url = postgres_url_or_default(clio_config::db_url_env::database_url_from_process_env());
        let Ok(mut holder) = crate::pg_tls::connect(&url) else {
            return;
        };
        holder
            .query_one("SELECT pg_advisory_lock($1)", &[&PG_TEST_PROBE_KEY])
            .expect("hold the probe advisory lock");

        let (tx, rx) = mpsc::channel();
        let waiter = std::thread::spawn(move || {
            let acquired = advisory_lock(&url, PG_TEST_PROBE_KEY).is_some();
            let _ = tx.send(acquired);
        });

        assert!(
            rx.recv_timeout(Duration::from_millis(300)).is_err(),
            "a second holder must wait while the lock is held"
        );
        drop(holder);
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(10)),
            Ok(true),
            "the waiter must acquire the lock once it is released"
        );
        waiter.join().expect("join lock waiter");
    }

    /// The probe key must not collide with the key every shared-Postgres test
    /// takes, or the test above is racy again.
    #[test]
    fn the_probe_key_is_distinct_from_the_shared_key() {
        assert_ne!(PG_TEST_PROBE_KEY, PG_TEST_ADVISORY_KEY);
    }

    /// A helper that opens the shared database must hand its guard to the
    /// caller, and the lock must stay held for the caller's whole body.
    ///
    /// This is the regression test for a real CI deadlock. `open_pg()` used to
    /// bind `let _guard = lock_pg_test()` locally, so the guard was dropped the
    /// moment the helper returned: every caller in that file then ran its
    /// queries against the shared database with no lock at all, and a belief
    /// correction deadlocked against a store open elsewhere whose DDL needs an
    /// exclusive table lock. The guard's type cannot catch that, because
    /// `let _guard = ...` in a helper satisfies `#[must_use]` perfectly.
    ///
    /// The probe runs on its own connection while the helper's caller is
    /// mid-test, so a guard that dies early shows up as an acquirable key.
    #[test]
    fn a_guard_handed_to_the_caller_stays_held_for_the_whole_test() {
        let url = postgres_url_or_default(clio_config::db_url_env::database_url_from_process_env());
        if crate::pg_tls::connect(&url).is_err() {
            eprintln!("skip: shared Postgres unreachable");
            return;
        }
        let (tx, rx) = mpsc::channel();
        // Stand in for a test body: take the guard through the helper, then hold
        // it while the probe looks for the key.
        let caller = std::thread::spawn(move || {
            let (_guard, _store, _kms) = crate::postgres_belief::postgres_belief_context_tests::open_pg();
            tx.send(()).expect("signal probe start");
            std::thread::sleep(Duration::from_millis(750));
        });
        rx.recv_timeout(Duration::from_secs(30))
            .expect("helper returned a guard");

        let mut probe = crate::pg_tls::connect(&url).expect("probe connect");
        let contended: bool = probe
            .query_one("SELECT pg_try_advisory_lock($1)", &[&PG_TEST_ADVISORY_KEY])
            .expect("probe try-lock")
            .get(0);
        assert!(
            !contended,
            "the shared-Postgres lock was free while a guard was still in scope: \
             a helper bound the guard locally instead of returning it"
        );
        drop(probe);
        caller.join().expect("join caller");
    }
}
