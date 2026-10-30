// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This integration target is responsible for proving the cross-path compliance
//! behaviour of one deployment: that export, erasure, restore, peer
//! reconciliation, archive admission, and review export each keep their
//! intended order and each refuse the unsafe state a fault leaves behind.
//!
//! ## Owns
//! - The module tree of the cross-path compliance proof
//! - The scenario modules, one per proven cross-path claim
//!
//! ## Does not own
//! - The shared fixtures, the CLI driver, the refusal harness, and the Postgres
//!   gate (see `case_fixtures`, `cli_test_support`, `refusals_test_support`, and
//!   `support_pg_test_support`)
//! - The production export, erase, and restore primitives (see `clio-compliance`)
//!
//! ## Boundary
//!
//! Changes to which cross-path claims are proven should normally require changes
//! here. Changes to a compliance primitive should not.
//!
//! Keep this target focused on proving the paths agree, not on the primitives it
//! drives.

#[path = "../common/mod.rs"]
mod common;
#[path = "../sync_wire_support/mod.rs"]
mod sync_wire_support;

mod case_fixtures;
mod cli_test_support;
mod family_fixtures;
mod family_test_support;
mod peer_process_test_support;
mod peer_test_support;
mod refusals_test_support;
mod review_export_test_support;
mod support_pg_test_support;
mod support_test_support;

mod scenario_all_bank_peer;
mod scenario_all_bank_peer_test_support;
mod scenario_archive_artifact_isolation;
mod scenario_archive_presence;
mod scenario_archive_presence_test_support;
mod scenario_closure_unknown_path;
mod scenario_event_review;
mod scenario_export_erase_restore;
mod scenario_intent_outcome_faults;
mod scenario_intent_outcome_faults_pending_restore;
mod scenario_intent_outcome_faults_reconcile;
mod scenario_intent_outcome_faults_review_outcome;
mod scenario_refusals_incomplete;
mod scenario_refusals_unknown_custody;
mod scenario_review_export;
