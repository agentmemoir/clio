// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for telling live listeners that the server's
//! tool view changed: one installed snapshot, one bounded subscriber set, and
//! one change counter that a waiting listener observes.
//!
//! ## Owns
//! - The installed view snapshot and the change counter derived from it
//! - The bounded subscriber registry, keyed by a monotonic serial
//! - The condition a listener waits on, with a bounded timeout
//! - The refusal a transport reports when the registry is full
//!
//! ## Does not own
//! - Building a snapshot from the live configuration (see `tools_page`)
//! - The snapshot value and its digest inputs (see `tool_snapshot`)
//! - Transport framing, request routing, or protocol capability
//!
//! ## Boundary
//!
//! Changes to how a change is announced, counted, or bounded should normally
//! require changes here. Changes to what a view digest covers should not
//! require changes here.
//!
//! Keep this module focused on the in-process change signal.

use crate::tool_snapshot::ToolSnapshot;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

/// The most listeners one server holds at the same time.
pub(crate) const MAX_SUBSCRIPTIONS: usize = 8;

/// One listener's view of the change counter.
#[derive(Debug)]
pub(crate) struct Subscriber {
    /// The change count this listener already observed.
    seen: Mutex<u64>,
    /// Signals that the count moved.
    ready: Condvar,
}

impl Subscriber {
    /// A subscriber that starts from `seen`.
    fn new(seen: u64) -> Self {
        Self {
            seen: Mutex::new(seen),
            ready: Condvar::new(),
        }
    }

    /// The change count this subscriber currently holds.
    pub(crate) fn observed(&self) -> u64 {
        *self.seen.lock().expect("subscriber mutex poisoned")
    }

    /// Wait until the change count differs from the last observed value.
    ///
    /// Returns the observed count when it differs, or the unchanged count when
    /// `timeout` expires. A spurious wake loops and costs nothing.
    #[cfg(test)]
    pub(crate) fn wait(&self, timeout: Duration) -> u64 {
        self.wait_from(self.observed(), timeout)
    }

    /// Wait until the count differs from `last`.
    ///
    /// A change published before this call returns at once, so a listener
    /// never waits a full timeout for a change it already holds.
    pub(crate) fn wait_from(&self, last: u64, timeout: Duration) -> u64 {
        let mut seen: std::sync::MutexGuard<'_, u64> =
            self.seen.lock().expect("subscriber mutex poisoned");
        loop {
            if *seen != last {
                return *seen;
            }
            let (guard, outcome) = self
                .ready
                .wait_timeout(seen, timeout)
                .expect("subscriber mutex poisoned");
            seen = guard;
            if *seen != last || outcome.timed_out() {
                return *seen;
            }
        }
    }
}

/// The snapshot and change counter one critical section guards.
#[derive(Default)]
struct SignalState {
    /// The snapshot the server installed, if any.
    snapshot: Option<Arc<ToolSnapshot>>,
    /// The number of installed changes after the seed.
    changes: u64,
}

/// The tool-view change signal for live listeners.
#[derive(Default)]
pub(crate) struct ToolSignals {
    /// The installed snapshot and its change counter.
    state: Mutex<SignalState>,
    /// The registered listeners, keyed by a monotonic serial.
    subscribers: Mutex<BTreeMap<u64, Arc<Subscriber>>>,
    /// The serial the next registration takes.
    next_serial: AtomicU64,
}

impl ToolSignals {
    /// The installed snapshot, if one exists.
    pub(crate) fn current(&self) -> Option<Arc<ToolSnapshot>> {
        self.state
            .lock()
            .expect("signal state mutex poisoned")
            .snapshot
            .clone()
    }

    /// Install the first snapshot without counting it as a change.
    ///
    /// A snapshot that another caller installed first wins; this call then
    /// returns that one. Seeding never moves the counter and never wakes a
    /// listener.
    pub(crate) fn seed(&self, next: ToolSnapshot) -> Arc<ToolSnapshot> {
        let mut state = self.state.lock().expect("signal state mutex poisoned");
        if let Some(installed) = &state.snapshot {
            return Arc::clone(installed);
        }
        let installed = Arc::new(next);
        state.snapshot = Some(Arc::clone(&installed));
        installed
    }

    /// Install `next` when it differs from the current snapshot.
    ///
    /// Returns the installed snapshot and wakes every listener, all after one
    /// critical section moved the counter. Returns `None` when the digest is
    /// already installed: nothing is swapped, the counter does not move, and
    /// nobody is woken. With no snapshot yet, this seeds and returns `None`.
    pub(crate) fn replace(&self, next: ToolSnapshot) -> Option<Arc<ToolSnapshot>> {
        let (installed, current) = {
            let mut state = self.state.lock().expect("signal state mutex poisoned");
            if let Some(existing) = &state.snapshot {
                if existing.digest() == next.digest() {
                    return None;
                }
            } else {
                state.snapshot = Some(Arc::new(next));
                return None;
            }
            let installed = Arc::new(next);
            state.snapshot = Some(Arc::clone(&installed));
            state.changes += 1;
            (installed, state.changes)
        };
        self.wake(current);
        Some(installed)
    }

    /// Register one listener and return its serial and the condition it waits
    /// on.
    ///
    /// Every registration inserts its own entry, so two requests never share
    /// one. A full registry refuses the registration with
    /// [`NotRegistered::Capacity`].
    pub(crate) fn register(&self) -> Result<(u64, Arc<Subscriber>), NotRegistered> {
        let current = self
            .state
            .lock()
            .expect("signal state mutex poisoned")
            .changes;
        let mut subscribers: std::sync::MutexGuard<'_, BTreeMap<u64, Arc<Subscriber>>> = self
            .subscribers
            .lock()
            .expect("subscriber registry mutex poisoned");
        if subscribers.len() >= MAX_SUBSCRIPTIONS {
            return Err(NotRegistered::Capacity);
        }
        let serial = self.next_serial.fetch_add(1, Ordering::Relaxed);
        let subscriber = Arc::new(Subscriber::new(current));
        subscribers.insert(serial, Arc::clone(&subscriber));
        Ok((serial, subscriber))
    }

    /// Move every registered listener to `current`, then wake it.
    fn wake(&self, current: u64) {
        let subscribers = self
            .subscribers
            .lock()
            .expect("subscriber registry mutex poisoned");
        for subscriber in subscribers.values() {
            *subscriber.seen.lock().expect("subscriber mutex poisoned") = current;
            subscriber.ready.notify_all();
        }
    }

    /// Drop one listener by its serial, and only that listener.
    pub(crate) fn unregister(&self, serial: u64) {
        self.subscribers
            .lock()
            .expect("subscriber registry mutex poisoned")
            .remove(&serial);
    }

    /// Publish one digest as a change, for tests of the signal itself.
    #[cfg(test)]
    pub(crate) fn publish(&self, digest: &str) -> bool {
        self.replace(ToolSnapshot::with_digest(digest)).is_some()
    }

    /// The number of registered listeners.
    #[cfg(test)]
    pub(crate) fn subscribers(&self) -> usize {
        self.subscribers
            .lock()
            .expect("subscriber registry mutex poisoned")
            .len()
    }
}

/// Why the registry refused a registration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NotRegistered {
    /// The registry already holds [`MAX_SUBSCRIPTIONS`] listeners.
    Capacity,
}

impl std::fmt::Display for NotRegistered {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Capacity => write!(
                formatter,
                "the server already holds {MAX_SUBSCRIPTIONS} tool-view subscriptions"
            ),
        }
    }
}

impl std::error::Error for NotRegistered {}

#[cfg(test)]
#[path = "tool_signals_tests.rs"]
mod tool_signals_tests;
