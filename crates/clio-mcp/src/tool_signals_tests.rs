// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for tests of the in-process tool-view change
//! signal: one registered subscriber per identifier, bounded capacity, the
//! seeded baseline, and the wake a publish delivers.
//!
//! ## Owns
//! - Assertions over the published digest, the change counter, and capacity
//! - The short wait every blocking assertion uses
//!
//! ## Does not own
//! - Production signaling behavior (see `tool_signals`)
//! - The tool-view digest (see `tools_page`)
//!
//! ## Boundary
//!
//! Changes to the change signal's contract should normally require changes
//! here. Changes to digest inputs should not.
//!
//! Keep this module focused on change-signal tests.

use super::{MAX_SUBSCRIPTIONS, NotRegistered, Subscriber, ToolSignals};
use std::sync::Arc;
use std::time::Duration;

/// A wait short enough to keep the suite fast and long enough that a missed
/// wake reports the unchanged count instead of hanging.
const SHORT_WAIT: Duration = Duration::from_millis(50);

fn signals() -> Arc<ToolSignals> {
    Arc::new(ToolSignals::default())
}

fn registered(signals: &ToolSignals, id: &str, baseline: &str) -> Arc<Subscriber> {
    signals
        .register(id, baseline.to_owned())
        .expect("registration below capacity")
}

#[test]
fn publish_wakes_every_registered_subscriber_once() {
    let signals = signals();
    let first = registered(&signals, "first", "seed");
    let second = registered(&signals, "second", "seed");

    assert!(signals.publish("view-1"));
    assert_eq!(first.wait(SHORT_WAIT), 1, "first listener sees one change");
    assert_eq!(
        second.wait(SHORT_WAIT),
        1,
        "second listener sees one change"
    );

    assert!(signals.publish("view-2"));
    assert_eq!(first.wait(SHORT_WAIT), 2, "first listener rises by one");
    assert_eq!(second.wait(SHORT_WAIT), 2, "second listener rises by one");
}

#[test]
fn republishing_the_same_digest_is_quiet() {
    let signals = signals();
    let subscriber = registered(&signals, "one", "seed");

    assert!(signals.publish("view-1"));
    assert_eq!(subscriber.wait(SHORT_WAIT), 1);

    assert!(!signals.publish("view-1"), "same digest is no change");
    assert_eq!(
        subscriber.wait(SHORT_WAIT),
        1,
        "the count does not move on a repeat"
    );
}

#[test]
fn repeated_registration_keeps_one_entry() {
    let signals = signals();
    let first = registered(&signals, "same", "seed");
    let second = registered(&signals, "same", "another");

    assert!(
        Arc::ptr_eq(&first, &second),
        "the same subscriber comes back"
    );
    assert_eq!(signals.subscribers(), 1);

    assert!(signals.publish("view-1"));
    assert_eq!(first.wait(SHORT_WAIT), 1, "one delivery for one identifier");
}

#[test]
fn unregister_stops_delivery() {
    let signals = signals();
    let subscriber = registered(&signals, "leaving", "seed");

    signals.unregister("leaving");
    assert_eq!(signals.subscribers(), 0);

    assert!(signals.publish("view-1"));
    assert_eq!(
        subscriber.wait(SHORT_WAIT),
        0,
        "an unregistered subscriber is not woken"
    );
}

#[test]
fn registration_stops_at_capacity() {
    let signals = signals();
    for index in 0..MAX_SUBSCRIPTIONS {
        registered(&signals, &format!("listener-{index}"), "seed");
    }
    assert_eq!(signals.subscribers(), MAX_SUBSCRIPTIONS);

    let refused = signals.register("overflow", "seed".to_owned());
    assert!(
        matches!(refused, Err(NotRegistered::Capacity)),
        "a full registry refuses a new identifier"
    );
    assert!(
        signals.register("listener-0", "seed".to_owned()).is_ok(),
        "an existing identifier still succeeds at the cap"
    );
    assert_eq!(signals.subscribers(), MAX_SUBSCRIPTIONS);
}

#[test]
fn wait_returns_the_unchanged_count_after_the_timeout() {
    let signals = signals();
    let subscriber = registered(&signals, "waiter", "seed");

    assert_eq!(
        subscriber.wait(SHORT_WAIT),
        0,
        "no publish means the same count"
    );
}

#[test]
fn first_registration_seeds_the_baseline() {
    let signals = signals();
    let subscriber = registered(&signals, "seeded", "baseline-digest");

    assert!(
        !signals.publish("baseline-digest"),
        "the seeded digest is not a change"
    );
    assert_eq!(subscriber.wait(SHORT_WAIT), 0);

    assert!(
        signals.publish("changed-digest"),
        "a new digest is a change"
    );
    assert_eq!(subscriber.wait(SHORT_WAIT), 1);
}

#[test]
fn publish_without_a_subscriber_only_seeds_the_baseline() {
    let signals = signals();

    assert!(!signals.publish("seed"), "the first publish seeds only");
    let subscriber = registered(&signals, "late", "seed");
    assert_eq!(subscriber.wait(SHORT_WAIT), 0);

    assert!(signals.publish("view-1"));
    assert_eq!(subscriber.wait(SHORT_WAIT), 1);
}
