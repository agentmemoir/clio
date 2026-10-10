// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the process-wide monotonic suffix for
//! hygiene audit-event ids so same-millisecond mutations on one item never
//! collide on the `audit_events` primary key.
//!
//! ## Owns
//! - The process-wide monotonic counter and [`next`]
//!
//! ## Does not own
//! - Hygiene transaction SQL or audit-event id composition
//!   (`sqlite_hygiene` / `postgres_hygiene`)
//!
//! ## Boundary
//!
//! Changes to sequence generation should normally require changes here.
//! Changes to event id formats should not require changes here.
//!
//! Keep this module focused on the monotonic hygiene sequence.

use std::sync::atomic::{AtomicU64, Ordering};

static SEQ: AtomicU64 = AtomicU64::new(0);

/// Next monotonic sequence number (wraps at u64::MAX; ids also carry the
/// millisecond timestamp, so wraparound collisions are not realistic).
pub(crate) fn next() -> u64 {
    SEQ.fetch_add(1, Ordering::Relaxed)
}
