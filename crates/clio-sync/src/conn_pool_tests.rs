// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the bounded connection pool: slot acquisition up to the
//! cap, immediate `None` above it, a pool of one behaving as a semaphore, and
//! release through the `Drop` guard on the exit paths exercised here: a
//! dropped guard, an early return of an error, and an unwinding panic in the
//! holder's scope.
//!
//! ## Owns
//! - Unit coverage for `conn_pool`
//!
//! ## Does not own
//! - The accept loop behavior that uses the pool (`server_conn_tests`)
//!
//! ## Boundary
//!
//! Tests here break when slot accounting changes; they never change it.
//!
//! Keep this module focused on connection-pool tests.

use super::{ConnectionPool, MAX_CONNECTIONS};
use std::panic::{AssertUnwindSafe, catch_unwind};

#[test]
fn pool_refuses_above_the_cap_and_releases_on_drop() {
    let pool = ConnectionPool::new(MAX_CONNECTIONS);
    let mut slots = Vec::new();
    for _ in 0..MAX_CONNECTIONS {
        slots.push(pool.try_acquire().expect("slot under the cap"));
    }
    assert!(
        pool.try_acquire().is_none(),
        "a full pool must refuse immediately, never block"
    );
    // Release by dropping every guard: the freed slots are acquirable again.
    slots.clear();
    for _ in 0..MAX_CONNECTIONS {
        slots.push(pool.try_acquire().expect("dropped guards release slots"));
    }
    assert!(pool.try_acquire().is_none());
}

#[test]
fn pool_slot_is_released_when_the_holder_returns_an_error() {
    // A pool of one, so the single post-error probe fails if the error path
    // leaked its slot; with a pool of eight it would pass either way.
    let pool = ConnectionPool::new(1);
    let serve = || -> Result<(), ()> {
        let _slot = pool.try_acquire().expect("slot");
        Err(()) // any exit path still unwinds the guard
    };
    assert!(serve().is_err(), "the holder returns an error");
    assert!(
        pool.try_acquire().is_some(),
        "an error return must release the slot via Drop"
    );
}

/// A panic in the holder's scope unwinds through the `Drop` guard, so the slot
/// is released instead of leaking a permanent connection-cap hole. The panic
/// is contained by `catch_unwind` (it prints a backtrace to stderr, which is
/// expected) so the test binary survives and the assertion below still runs.
#[test]
fn pool_slot_is_released_when_the_holder_panics() {
    let pool = ConnectionPool::new(1);
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        let _slot = pool.try_acquire().expect("slot");
        panic!("boom");
    }));
    assert!(outcome.is_err(), "the holder's scope must unwind");
    assert!(
        pool.try_acquire().is_some(),
        "an unwinding panic must release the slot via Drop"
    );
}

#[test]
fn a_pool_of_one_behaves_as_a_semaphore() {
    let pool = ConnectionPool::new(1);
    let first = pool.try_acquire().expect("first slot");
    assert!(pool.try_acquire().is_none());
    drop(first);
    assert!(pool.try_acquire().is_some());
}
