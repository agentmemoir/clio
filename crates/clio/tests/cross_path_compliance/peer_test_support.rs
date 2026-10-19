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
//! - The report's own success flag and peer-name reading
//!
//! ## Does not own
//! - The peer processes and their durable rows (see `peer_support`)
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

/// The peer names one peer-status report carries, without its own success flag.
fn named_peers(report: &Value) -> Vec<&str> {
    let object = report.as_object().expect("the peer report is an object");
    object
        .keys()
        .map(String::as_str)
        .filter(|key| *key != OK_KEY)
        .collect()
}

/// Assert a peers-disabled deployment's peer-status report names no peer.
///
/// The report must be a non-empty object carrying its own success flag, so an
/// empty or missing report fails instead of passing as "no peer named".
pub fn assert_no_peer_named(report: &Value) {
    let object = report.as_object().expect("the peer report is an object");
    assert!(
        object.contains_key(OK_KEY),
        "the peer report carries its own success flag: {report}"
    );
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
    let object = report.as_object().expect("the peer report is an object");
    assert!(
        object.contains_key(OK_KEY),
        "the peer report carries its own success flag: {report}"
    );
    let mut named = named_peers(report);
    named.sort_unstable();
    let mut want = expected.to_vec();
    want.sort_unstable();
    assert_eq!(
        named, want,
        "the peer report names exactly the frozen peers: {report}"
    );
}
