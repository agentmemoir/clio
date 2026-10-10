// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for bounding concurrent HTTP transport
//! connections with a fixed-size pool: at most 32 connections hold a slot at
//! once, acquisition never blocks, and every slot is released on every exit
//! path.
//!
//! ## Owns
//! - The 32-connection cap constant
//! - Slot counting, non-blocking acquisition, and Drop-guaranteed release
//!
//! ## Does not own
//! - The accept loop and the 503 refusal above the cap (see `http`)
//! - Per-connection read/write timeouts and the request deadline (see `http`)
//!
//! ## Boundary
//!
//! Changes to how connection slots are counted or released should normally
//! require changes here. Changes to HTTP parsing, routing, or responses
//! should not require changes here.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Maximum concurrent HTTP connections the transport will serve at once.
///
/// Above the cap the accept loop answers a readable 503 and closes instead of
/// queueing unbounded work behind slow clients. The value is recorded and
/// pinned by a test; raising it silently is a defect.
pub(crate) const MAX_CONNECTIONS: usize = 32;

/// Counting semaphore over an atomic counter (`std` has no semaphore type).
///
/// `try_acquire` never blocks: a full pool returns `None` immediately so the
/// accept loop can refuse the connection instead of waiting behind slow
/// clients.
pub(crate) struct ConnPool {
    /// Slots currently held; never exceeds `MAX_CONNECTIONS`.
    in_use: AtomicUsize,
}

/// RAII slot guard.
///
/// Release happens on Drop, never by a manual call on each path: success, a
/// client error, a server error, a write failure, a timeout, or a panic all
/// unwind through this guard. A leaked slot would convert one slow client
/// into a permanent outage.
pub(crate) struct PoolGuard {
    pool: Arc<ConnPool>,
}

impl Default for ConnPool {
    fn default() -> Self {
        Self {
            in_use: AtomicUsize::new(0),
        }
    }
}

impl ConnPool {
    /// Take a slot if one is free; `None` when the pool is full.
    ///
    /// The allow below sits on the one call rather than on this function, so
    /// that a different deprecated API added here later is still reported.
    ///
    /// `fetch_update` is deprecated in favour of `try_update`, which is a pure
    /// rename, but `try_update` is only stable from Rust 1.95 and this
    /// workspace declares an older minimum. The allow therefore covers two
    /// separate diagnostics: rustc's `deprecated`, which is what the rename
    /// causes, and clippy's `incompatible_msrv`, which is what the older
    /// minimum causes. Both go away together when the minimum moves to 1.95.
    pub(crate) fn try_acquire(self: &Arc<Self>) -> Option<PoolGuard> {
        #[allow(deprecated, clippy::incompatible_msrv)]
        let acquired = self
            .in_use
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < MAX_CONNECTIONS).then_some(n + 1)
            });
        acquired.ok().map(|_| PoolGuard {
            pool: Arc::clone(self),
        })
    }

    fn release(&self) {
        self.in_use.fetch_sub(1, Ordering::Release);
    }
}

impl Drop for PoolGuard {
    fn drop(&mut self) {
        self.pool.release();
    }
}

#[cfg(test)]
#[path = "conn_pool_tests.rs"]
mod conn_pool_tests;
#[cfg(test)]
#[path = "conn_pool_transport_bounds_tests.rs"]
mod conn_pool_transport_bounds_tests;
