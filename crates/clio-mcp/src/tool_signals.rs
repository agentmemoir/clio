// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for telling live listeners that the server's
//! tool view changed: one bounded subscriber set, one published digest, and
//! one change counter that a waiting listener observes.
//!
//! ## Owns
//! - The published view digest and the change counter derived from it
//! - The bounded subscriber registry and its identifier deduplication
//! - The condition a listener waits on, with a bounded timeout
//! - The refusal a transport reports when the registry is full
//!
//! ## Does not own
//! - Computing the tool-view digest (see `tools_page`)
//! - Transport framing, request routing, or protocol capability
//!
//! ## Boundary
//!
//! Changes to how a change is announced, counted, or bounded should normally
//! require changes here. Changes to what a view digest covers should not
//! require changes here.
//!
//! Keep this module focused on the in-process change signal.

use std::collections::BTreeMap;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

/// The most listeners one server holds at the same time.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) const MAX_SUBSCRIPTIONS: usize = 8;

/// One listener's view of the change counter.
#[derive(Debug)]
pub(crate) struct Subscriber {
    /// The change count this listener already observed.
    seen: Mutex<u64>,
    /// Signals that the count moved.
    ready: Condvar,
}

#[cfg_attr(not(test), allow(dead_code))]
impl Subscriber {
    /// A subscriber that starts from `seen`.
    fn new(seen: u64) -> Self {
        Self {
            seen: Mutex::new(seen),
            ready: Condvar::new(),
        }
    }

    /// Wait until the change count differs from the last observed value.
    ///
    /// Returns the observed count when it differs, or the unchanged count when
    /// `timeout` expires. A spurious wake loops and costs nothing.
    pub(crate) fn wait(&self, timeout: Duration) -> u64 {
        let mut seen: std::sync::MutexGuard<'_, u64> =
            self.seen.lock().expect("subscriber mutex poisoned");
        let baseline = *seen;
        loop {
            let (guard, outcome) = self
                .ready
                .wait_timeout(seen, timeout)
                .expect("subscriber mutex poisoned");
            seen = guard;
            if *seen != baseline || outcome.timed_out() {
                return *seen;
            }
        }
    }
}

/// The tool-view change signal for live listeners.
#[derive(Default)]
pub(crate) struct ToolSignals {
    /// The digest of the view the server last announced.
    published: Mutex<Option<String>>,
    /// The number of announced changes.
    changes: Mutex<u64>,
    /// The registered listeners, keyed by their identifier.
    subscribers: Mutex<BTreeMap<String, Arc<Subscriber>>>,
}

impl ToolSignals {
    /// Register one listener and return the condition it waits on.
    ///
    /// The first registration seeds the published baseline and notifies
    /// nobody. A repeated identifier returns the subscriber already registered.
    /// A full registry refuses a new identifier with [`NotRegistered::Capacity`].
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn register(
        &self,
        id: &str,
        baseline: String,
    ) -> Result<Arc<Subscriber>, NotRegistered> {
        {
            let mut published: std::sync::MutexGuard<'_, Option<String>> = self
                .published
                .lock()
                .expect("published digest mutex poisoned");
            if published.is_none() {
                *published = Some(baseline);
            }
        }
        let current = *self.changes.lock().expect("change counter mutex poisoned");
        let mut subscribers: std::sync::MutexGuard<'_, BTreeMap<String, Arc<Subscriber>>> = self
            .subscribers
            .lock()
            .expect("subscriber registry mutex poisoned");
        if let Some(existing) = subscribers.get(id) {
            return Ok(Arc::clone(existing));
        }
        if subscribers.len() >= MAX_SUBSCRIPTIONS {
            return Err(NotRegistered::Capacity);
        }
        let subscriber = Arc::new(Subscriber::new(current));
        subscribers.insert(id.to_owned(), Arc::clone(&subscriber));
        Ok(subscriber)
    }

    /// Drop one listener, if it is registered.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn unregister(&self, id: &str) {
        self.subscribers
            .lock()
            .expect("subscriber registry mutex poisoned")
            .remove(id);
    }

    /// Publish one view digest and wake every listener when it is new.
    ///
    /// Returns false when the digest is already published, or when the first
    /// publish only seeds the baseline. Otherwise it stores the digest, moves
    /// the change counter, and wakes every listener.
    pub(crate) fn publish(&self, digest: &str) -> bool {
        {
            let mut published: std::sync::MutexGuard<'_, Option<String>> = self
                .published
                .lock()
                .expect("published digest mutex poisoned");
            match published.as_deref() {
                Some(current) if current == digest => return false,
                Some(_) => {}
                None => {
                    *published = Some(digest.to_owned());
                    return false;
                }
            }
            *published = Some(digest.to_owned());
        }
        let current = {
            let mut changes = self.changes.lock().expect("change counter mutex poisoned");
            *changes += 1;
            *changes
        };
        let subscribers: std::sync::MutexGuard<'_, BTreeMap<String, Arc<Subscriber>>> = self
            .subscribers
            .lock()
            .expect("subscriber registry mutex poisoned");
        for subscriber in subscribers.values() {
            *subscriber.seen.lock().expect("subscriber mutex poisoned") = current;
            subscriber.ready.notify_all();
        }
        true
    }

    /// The number of registered listeners.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn subscribers(&self) -> usize {
        self.subscribers
            .lock()
            .expect("subscriber registry mutex poisoned")
            .len()
    }
}

/// Why the registry refused a registration.
#[cfg_attr(not(test), allow(dead_code))]
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
