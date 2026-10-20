// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the peer-status report assertions of the
//! cross-path compliance proof: a peers-disabled deployment names no peer, and
//! a two-peer deployment names exactly the two frozen peers.
//!
//! ## Owns
//! - `assert_no_peer_named` and `assert_peer_slots`
//! - The report's success flag, its peer states, and the peer-name reading
//!
//! ## Does not own
//! - The peer processes and their durable rows (see `peer_process_test_support`)
//! - The scenarios that call these assertions (see the `scenario_*` modules)
//!
//! ## Boundary
//!
//! Changes to what a peer-status report must carry should normally require
//! changes here. Changes to a scenario's assertions should not.
//!
//! Keep this module focused on the peer-status report checks.

use serde_json::Value;

/// The key the peer-status report carries for its own success, beside the
/// peer names.
const OK_KEY: &str = "ok";

/// The peer states the production reports for one peer slot.
const PEER_STATES: [&str; 3] = ["pending", "acknowledged", "disposed"];

/// Assert the report is the production's success envelope.
///
/// A report that says `ok: false` is a failure envelope, not a peer list, so
/// it fails here instead of having its error fields read as peer names.
fn assert_success(report: &Value) {
    let object = report.as_object().expect("the peer report is an object");
    assert_eq!(
        object.get(OK_KEY).and_then(Value::as_bool),
        Some(true),
        "the peer report carries its own success flag: {report}"
    );
}

/// The peer names one peer-status report carries, without its own success flag.
///
/// Every key beside the success flag names a peer slot, so a key that carries
/// no peer state names something else: the report is refused rather than
/// reading that key as a peer.
fn named_peers(report: &Value) -> Vec<&str> {
    let object = report.as_object().expect("the peer report is an object");
    let mut named: Vec<&str> = Vec::new();
    for (key, value) in object {
        if key.as_str() == OK_KEY {
            continue;
        }
        let state = value.as_str().unwrap_or_else(|| {
            panic!("the peer report's key {key} carries no peer state: {report}")
        });
        assert!(
            PEER_STATES.contains(&state),
            "the peer report's key {key} carries no peer state: {report}"
        );
        named.push(key.as_str());
    }
    named
}

/// Assert a peers-disabled deployment's peer-status report names no peer.
///
/// The report must be the production's success envelope, so an empty, a
/// missing, or a failed report fails instead of passing as "no peer named".
pub fn assert_no_peer_named(report: &Value) {
    assert_success(report);
    let named = named_peers(report);
    assert!(
        named.is_empty(),
        "a peers-disabled deployment names no peer: {named:?}"
    );
}

/// Assert a peer-status report names exactly the given peers.
///
/// A missing peer fails: the named set must equal the frozen set, so a report
/// that drops one peer is refused rather than accepted.
pub fn assert_peer_slots(report: &Value, expected: &[&str]) {
    assert_success(report);
    let mut named = named_peers(report);
    named.sort_unstable();
    let mut want = expected.to_vec();
    want.sort_unstable();
    assert_eq!(
        named, want,
        "the peer report names exactly the frozen peers: {report}"
    );
}
