// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for tests of the in-process tool-view change
//! signal: one registered subscriber per registration, bounded capacity, the
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

fn registered(signals: &ToolSignals) -> (u64, Arc<Subscriber>) {
    signals.register().expect("registration below capacity")
}

#[test]
fn publish_wakes_every_registered_subscriber_once() {
    let signals = signals();
    assert!(!signals.publish("seed"), "the first publish seeds only");
    let (_, first) = registered(&signals);
    let (_, second) = registered(&signals);

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
    assert!(!signals.publish("seed"), "the first publish seeds only");
    let (_, subscriber) = registered(&signals);

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
fn each_registration_keeps_its_own_entry_and_unregisters_only_itself() {
    let signals = signals();
    assert!(!signals.publish("seed"), "the first publish seeds only");
    let (first_serial, first) = registered(&signals);
    let (second_serial, second) = registered(&signals);

    assert!(!Arc::ptr_eq(&first, &second), "each stream owns its entry");
    assert_eq!(signals.subscribers(), 2);

    signals.unregister(first_serial);
    assert_eq!(signals.subscribers(), 1, "only the first entry is removed");

    assert!(signals.publish("view-1"));
    assert_eq!(
        second.wait(SHORT_WAIT),
        1,
        "the remaining subscriber sees the change"
    );

    signals.unregister(second_serial);
    assert_eq!(signals.subscribers(), 0);
}

#[test]
fn unregister_stops_delivery() {
    let signals = signals();
    assert!(!signals.publish("seed"), "the first publish seeds only");
    let (serial, subscriber) = registered(&signals);

    signals.unregister(serial);
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
    for _ in 0..MAX_SUBSCRIPTIONS {
        registered(&signals);
    }
    assert_eq!(signals.subscribers(), MAX_SUBSCRIPTIONS);

    let refused = signals.register();
    assert!(
        matches!(refused, Err(NotRegistered::Capacity)),
        "a full registry refuses another registration"
    );
    assert_eq!(signals.subscribers(), MAX_SUBSCRIPTIONS);
}

#[test]
fn wait_returns_the_unchanged_count_after_the_timeout() {
    let signals = signals();
    let (_, subscriber) = registered(&signals);

    assert_eq!(
        subscriber.wait(SHORT_WAIT),
        0,
        "no publish means the same count"
    );
}

#[test]
fn the_first_publish_seeds_the_baseline() {
    let signals = signals();
    let (_, subscriber) = registered(&signals);

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
    let (_, subscriber) = registered(&signals);
    assert_eq!(subscriber.wait(SHORT_WAIT), 0);

    assert!(signals.publish("view-1"));
    assert_eq!(subscriber.wait(SHORT_WAIT), 1);
}
