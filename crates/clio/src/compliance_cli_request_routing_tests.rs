// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the request and peer-request routing seam: the verb list
//! the `compliance` dispatcher routes to `compliance_request_cli` and the
//! registry's `request-*` and `peer-*` subset stay equal.
//!
//! ## Owns
//! - The parity assertion between the routed request/peer verb list and the
//!   registry subset derived from `compliance_subcommands()`
//!
//! ## Does not own
//! - The dispatch itself (`compliance_cli`) or the registry lookups
//!   (`cli_command_registry`)
//! - The dispatcher-completeness scan (`cli_help_dispatch_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the request and peer-request routing parity.

use super::*;

/// The request and peer-request verbs the `compliance` dispatcher routes to
/// `compliance_request_cli`: the registry's `request-*` and `peer-*` subset.
/// The parity test pins this list against that subset, so the routed set cannot
/// drift from the registry.
const REQUEST_SUBCOMMANDS: &[&str] = &[
    "request-start",
    "request-status",
    "request-retry",
    "request-copy",
    "request-exception",
    "request-events",
    "request-close",
    "peer-status",
    "peer-retry",
    "peer-disposition",
];

/// The routed request and peer-request verb list equals the registry's
/// `request-*` and `peer-*` subset, so a verb added to the registry cannot go
/// unrouted and a removed route cannot leave the routed set behind.
#[test]
fn request_subcommands_match_the_registry_request_and_peer_subset() {
    let mut derived: Vec<&str> = compliance_subcommands()
        .into_iter()
        .filter(|sub| sub.starts_with("request-") || sub.starts_with("peer-"))
        .collect();
    derived.sort_unstable();
    let mut guard = REQUEST_SUBCOMMANDS.to_vec();
    guard.sort_unstable();
    assert_eq!(
        guard, derived,
        "the routed verb list matches the registry subset"
    );
}
