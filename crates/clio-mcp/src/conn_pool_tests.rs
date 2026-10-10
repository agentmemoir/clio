// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module unit-tests the bounded connection pool: the 32-slot cap, the
//! immediate `None` above it, and release through the `Drop` guard on every
//! exit path exercised here — a dropped guard, an error return, and an
//! unwinding panic in the holder's scope.
//!
//! ## Owns
//! - Unit coverage for `conn_pool`
//!
//! ## Does not own
//! - The accept loop and the 503 refusal above the cap (`transport_bounds_tests`)
//! - Slot counting and the `Drop` release themselves (see `conn_pool`)
//!
//! ## Boundary
//!
//! Tests here break when slot accounting changes; they never change it.
//!
//! Keep this module focused on connection-pool unit tests. The transport-level
//! behavior of the same cap is covered over real sockets elsewhere.

use super::{ConnPool, MAX_CONNECTIONS};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

#[test]
fn pool_slots_are_bounded_and_drop_releases() {
    let pool = Arc::new(ConnPool::default());
    let mut guards = Vec::new();
    for _ in 0..MAX_CONNECTIONS {
        guards.push(pool.try_acquire().expect("slot within cap"));
    }
    assert!(
        pool.try_acquire().is_none(),
        "the pool must refuse acquisition above the cap"
    );
    drop(guards.pop().expect("guard"));
    assert!(
        pool.try_acquire().is_some(),
        "a dropped guard must release its slot"
    );
}

#[test]
fn pool_slots_survive_churn_without_leaks() {
    let pool = Arc::new(ConnPool::default());
    for _ in 0..(MAX_CONNECTIONS * 4) {
        drop(pool.try_acquire().expect("slot"));
    }
    let mut guards = Vec::new();
    for _ in 0..MAX_CONNECTIONS {
        guards.push(
            pool.try_acquire()
                .expect("no slot may leak across acquire/release churn"),
        );
    }
    assert!(pool.try_acquire().is_none(), "cap must still hold");
}

/// The error-return path must release its slot. Every other slot is held, so
/// the probe below can only succeed if the returning holder released the one
/// it held: a leak here leaves the pool one short and the probe fails.
#[test]
fn pool_slot_is_released_when_the_holder_returns_an_error() {
    let pool = Arc::new(ConnPool::default());
    let held: Vec<_> = (0..MAX_CONNECTIONS - 1)
        .map(|_| pool.try_acquire().expect("slot below the cap"))
        .collect();
    let serve = || -> Result<(), ()> {
        let _slot = pool.try_acquire().expect("last slot");
        Err(()) // any exit path still unwinds the guard
    };
    assert!(serve().is_err(), "the holder returns an error");
    assert!(
        pool.try_acquire().is_some(),
        "an error return must release the slot via Drop"
    );
    drop(held);
}

/// A panic in the holder's scope unwinds through the `Drop` guard, so the slot
/// is released instead of leaking a permanent connection-cap hole. The panic is
/// contained by `catch_unwind` (it prints a backtrace to stderr, which is
/// expected) so the test binary survives and the assertion below still runs.
#[test]
fn pool_slot_is_released_when_the_holder_panics() {
    let pool = Arc::new(ConnPool::default());
    // Hold every slot but one, so the probe below fails if the panicking
    // holder's slot is not released. With the whole cap held by the panicking
    // scope the assertion would pass for the wrong reason.
    let held: Vec<_> = (0..MAX_CONNECTIONS - 1)
        .map(|_| pool.try_acquire().expect("slot below the cap"))
        .collect();
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        let _slot = pool.try_acquire().expect("last slot");
        panic!("boom");
    }));
    assert!(outcome.is_err(), "the holder's scope must unwind");
    assert!(
        pool.try_acquire().is_some(),
        "an unwinding panic must release the slot via Drop"
    );
    // Probe once more through a second unwinding panic. One release can be lost
    // to a counting accident rather than to a real leak — a slot-releasing
    // mutant that skips an even-numbered release, say — and a single probe
    // would read that accident as a pass. Two independent releases, both
    // checked, leave a mutant no parity to hide behind.
    let second = catch_unwind(AssertUnwindSafe(|| {
        let _slot = pool.try_acquire().expect("a released slot must come back");
        panic!("boom again");
    }));
    assert!(second.is_err(), "the second holder's scope must unwind");
    assert!(
        pool.try_acquire().is_some(),
        "every unwinding panic must release its slot via Drop, not just one"
    );
    drop(held);
}
