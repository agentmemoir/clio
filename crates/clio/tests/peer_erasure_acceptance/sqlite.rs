// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for registering the peer-erasure acceptance
//! scenarios on the SQLite backend: both peer processes run the real
//! `clio sync serve` binary over loopback against their own scratch databases.
//!
//! ## Owns
//! - The SQLite registrations of every scenario
//!
//! ## Does not own
//! - The scenario bodies (`scenarios_peer`, `scenarios_disposition`)
//! - The Postgres registrations (`postgres`)
//!
//! ## Boundary
//!
//! Changes to which scenarios run on SQLite should normally require changes
//! here. Changes to what a scenario asserts should not.
//!
//! Keep this module focused on the SQLite test registrations.

use super::nodes::Node;
use super::run::Run;
use super::scenarios_disposition::{
    scenario_04, scenario_04_removal, scenario_05, scenario_06_disabled, scenario_06_invalid,
};
use super::scenarios_peer::scenario_01;
use super::scenarios_replay::{scenario_02, scenario_03};
use super::support::BANKS;

/// Scenario 01: freeze two peers, take one offline, erase, restart.
#[test]
fn sqlite_t01_offline_peer_and_restart_keep_the_pending_result() {
    let mut run = Run::sqlite("sqlite-s01");
    let a = Node::sqlite("sqlite-s01-a", &BANKS);
    let b = Node::sqlite("sqlite-s01-b", &BANKS);
    scenario_01(&mut run, &a, &b);
}

/// Scenario 02: reconnect denies before serving or applying; the other subject
/// still works.
#[test]
fn sqlite_t02_reconnect_denies_before_serving_or_applying() {
    let mut run = Run::sqlite("sqlite-s02");
    let a = Node::sqlite("sqlite-s02-a", &BANKS);
    let b = Node::sqlite("sqlite-s02-b", &BANKS);
    scenario_02(&mut run, &a, &b);
}

/// Scenario 03: repeated and concurrent delivery, restart before the
/// acknowledgement, one durable result per peer/action.
#[test]
fn sqlite_t03_duplicate_delivery_and_restart_reconcile_once() {
    let mut run = Run::sqlite("sqlite-s03");
    let a = Node::sqlite("sqlite-s03-a", &BANKS);
    let b = Node::sqlite("sqlite-s03-b", &BANKS);
    scenario_03(&mut run, &a, &b);
}

/// Scenario 04: retention and unknown dispositions are recorded and keep the
/// request open.
#[test]
fn sqlite_t04_retention_and_unknown_dispositions_stay_open() {
    let mut run = Run::sqlite("sqlite-s04");
    let a = Node::sqlite("sqlite-s04-a", &BANKS);
    let b = Node::sqlite("sqlite-s04-b", &BANKS);
    let c = Node::sqlite("sqlite-s04-c", &BANKS);
    scenario_04(&mut run, &a, &b, &c);
}

/// Scenario 04, removal arm: recorded deletion evidence settles the request.
#[test]
fn sqlite_t04_removal_evidence_settles_the_request() {
    let mut run = Run::sqlite("sqlite-s04-removal");
    let a = Node::sqlite("sqlite-s04-removal-a", &BANKS);
    scenario_04_removal(&mut run, &a);
}

/// Scenario 05: intent and outcome failures, restart, and pruning under a
/// pending peer.
#[test]
fn sqlite_t05_failed_intent_and_outcome_prune_preserves_safety_state() {
    let mut run = Run::sqlite("sqlite-s05");
    let a = Node::sqlite("sqlite-s05-a", &BANKS);
    let b = Node::sqlite("sqlite-s05-b", &BANKS);
    scenario_05(&mut run, &a, &b);
}

/// Scenario 06, disabled arm: the core runs with peers not applicable.
#[test]
fn sqlite_t06_peers_disabled_is_not_applicable() {
    let mut run = Run::sqlite("sqlite-s06-disabled");
    scenario_06_disabled(&mut run);
}

/// Scenario 06, invalid-result arm: no invalid result acknowledges or widens.
#[test]
fn sqlite_t06_invalid_results_cannot_acknowledge_or_widen_scope() {
    let mut run = Run::sqlite("sqlite-s06");
    let a = Node::sqlite("sqlite-s06-a", &BANKS);
    let b = Node::sqlite("sqlite-s06-b", &BANKS);
    let c = Node::sqlite("sqlite-s06-c", &BANKS);
    scenario_06_invalid(&mut run, &a, &b, &c);
}
