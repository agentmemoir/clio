// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the sync server's bounded connection pool:
//! a counting semaphore over connection slots so at most
//! [`MAX_CONNECTIONS`] connections are being served at a time, and every slot
//! is released on every exit path through a `Drop` guard.
//!
//! ## Owns
//! - The slot counter and its try-acquire / release semantics
//! - [`MAX_CONNECTIONS`], the stated connection cap (8)
//!
//! ## Does not own
//! - The accept loop and the 503 refusal a full pool produces (`server_conn`)
//! - How long one connection holds its slot (bounded by the request deadline,
//!   see `http` and `server_conn`)
//!
//! ## Boundary
//!
//! Changes to slot accounting or the cap value should normally require
//! changes here. Changes to refusal wording or status should not.
//!
//! Keep this module focused on bounded connection-slot accounting.

/// Maximum number of connections served at once. Protects the server from a
/// slow-client pile-up exhausting memory or threads: an accepted connection
/// above the cap is refused with a readable 503, never queued unbounded. A
/// slot is held from accept until the connection's handling finishes and is
/// released by a `Drop` guard on success, client error, server error, write
/// failure, timeout, or disconnect.
pub(crate) const MAX_CONNECTIONS: usize = 8;

/// A bounded pool of connection slots. `try_acquire` never blocks: when the
/// pool is full it returns `None` immediately and the caller refuses the
/// connection, so the accept loop cannot be stalled by slow clients.
pub(crate) struct ConnectionPool {
    inner: std::sync::Arc<PoolInner>,
}

struct PoolInner {
    max: usize,
    held: std::sync::Mutex<usize>,
}

/// One held connection slot. Released by `Drop`, which guarantees release on
/// every path (success, client error, server error, write failure, timeout,
/// panic) without manual bookkeeping at each call site.
pub(crate) struct PoolSlot {
    inner: std::sync::Arc<PoolInner>,
}

impl ConnectionPool {
    /// A pool with `max` slots.
    pub(crate) fn new(max: usize) -> Self {
        Self {
            inner: std::sync::Arc::new(PoolInner {
                max,
                held: std::sync::Mutex::new(0),
            }),
        }
    }

    /// Take one slot if one is free; `None` when the pool is full.
    pub(crate) fn try_acquire(&self) -> Option<PoolSlot> {
        let mut held = self
            .inner
            .held
            .lock()
            .expect("connection pool counter poisoned");
        if *held >= self.inner.max {
            return None;
        }
        *held += 1;
        Some(PoolSlot {
            inner: std::sync::Arc::clone(&self.inner),
        })
    }
}

impl Drop for PoolSlot {
    fn drop(&mut self) {
        let mut held = self
            .inner
            .held
            .lock()
            .expect("connection pool counter poisoned");
        *held -= 1;
    }
}

#[cfg(test)]
#[path = "conn_pool_tests.rs"]
mod conn_pool_tests;
