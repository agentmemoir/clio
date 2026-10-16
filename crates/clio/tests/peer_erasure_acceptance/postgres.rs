// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for registering the peer-erasure acceptance
//! scenarios on loopback Postgres: both peer processes run the real
//! `clio sync serve` binary against private schemas of the composed database,
//! serialized under the store's Postgres test lock.
//!
//! ## Owns
//! - The Postgres registrations of the scenarios that run on it
//!
//! ## Does not own
//! - The scenario bodies (`scenarios_peer`, `scenarios_disposition`)
//! - The SQLite registrations (`sqlite`)
//!
//! ## Boundary
//!
//! Changes to which scenarios run on Postgres should normally require changes
//! here. Changes to what a scenario asserts should not.
//!
//! Keep this module focused on the Postgres test registrations.

use super::nodes::Node;
use super::run::Run;
use super::scenarios_disposition::{
    scenario_04, scenario_04_removal, scenario_05, scenario_06_disabled, scenario_06_invalid,
};
use super::scenarios_peer::scenario_01;
use super::scenarios_replay::{scenario_02, scenario_03};
use super::support::BANKS;

/// The base URL a Postgres-backed run reports, or a skip when the operator
/// explicitly opted out of Postgres tests. `Run::postgres` already refused an
/// unreachable database unless the opt-out is set.
fn pg_base(run: &Run<clio::store::PostgresStore>) -> String {
    run.pg_base()
        .expect("a Postgres-backed run names its base URL")
        .to_owned()
}

/// Scenario 01 on loopback Postgres.
#[test]
fn postgres_t01_offline_peer_and_restart_keep_the_pending_result() {
    let Some(mut run) = Run::postgres("pg-s01") else {
        return;
    };
    let base = pg_base(&run);
    let a = Node::postgres("pg-s01-a", &base, &BANKS);
    let b = Node::postgres("pg-s01-b", &base, &BANKS);
    scenario_01(&mut run, &a, &b);
}

/// Scenario 02 on loopback Postgres.
#[test]
fn postgres_t02_reconnect_denies_before_serving_or_applying() {
    let Some(mut run) = Run::postgres("pg-s02") else {
        return;
    };
    let base = pg_base(&run);
    let a = Node::postgres("pg-s02-a", &base, &BANKS);
    let b = Node::postgres("pg-s02-b", &base, &BANKS);
    scenario_02(&mut run, &a, &b);
}

/// Scenario 03 on loopback Postgres.
#[test]
fn postgres_t03_duplicate_delivery_and_restart_reconcile_once() {
    let Some(mut run) = Run::postgres("pg-s03") else {
        return;
    };
    let base = pg_base(&run);
    let a = Node::postgres("pg-s03-a", &base, &BANKS);
    let b = Node::postgres("pg-s03-b", &base, &BANKS);
    scenario_03(&mut run, &a, &b);
}

/// Scenario 04 on loopback Postgres.
#[test]
fn postgres_t04_retention_and_unknown_dispositions_stay_open() {
    let Some(mut run) = Run::postgres("pg-s04") else {
        return;
    };
    let base = pg_base(&run);
    let a = Node::postgres("pg-s04-a", &base, &BANKS);
    let b = Node::postgres("pg-s04-b", &base, &BANKS);
    let c = Node::postgres("pg-s04-c", &base, &BANKS);
    scenario_04(&mut run, &a, &b, &c);
}

/// Scenario 04, removal arm, on loopback Postgres.
#[test]
fn postgres_t04_removal_evidence_settles_the_request() {
    let Some(mut run) = Run::postgres("pg-s04-removal") else {
        return;
    };
    let base = pg_base(&run);
    let a = Node::postgres("pg-s04-removal-a", &base, &BANKS);
    scenario_04_removal(&mut run, &a);
}

/// Scenario 05 on loopback Postgres.
#[test]
fn postgres_t05_failed_intent_and_outcome_prune_preserves_safety_state() {
    let Some(mut run) = Run::postgres("pg-s05") else {
        return;
    };
    let base = pg_base(&run);
    let a = Node::postgres("pg-s05-a", &base, &BANKS);
    let b = Node::postgres("pg-s05-b", &base, &BANKS);
    scenario_05(&mut run, &a, &b);
}

/// Scenario 06, disabled arm, on loopback Postgres.
#[test]
fn postgres_t06_peers_disabled_is_not_applicable() {
    let Some(mut run) = Run::postgres("pg-s06-disabled") else {
        return;
    };
    scenario_06_disabled(&mut run);
}

/// Scenario 06, invalid-result arm, on loopback Postgres.
#[test]
fn postgres_t06_invalid_results_cannot_acknowledge_or_widen_scope() {
    let Some(mut run) = Run::postgres("pg-s06") else {
        return;
    };
    let base = pg_base(&run);
    let a = Node::postgres("pg-s06-a", &base, &BANKS);
    let b = Node::postgres("pg-s06-b", &base, &BANKS);
    let c = Node::postgres("pg-s06-c", &base, &BANKS);
    scenario_06_invalid(&mut run, &a, &b, &c);
}
