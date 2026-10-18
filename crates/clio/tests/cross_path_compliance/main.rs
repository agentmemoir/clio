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
//! - The six scenario modules, one per acceptance claim
//!
//! ## Does not own
//! - The shared fixtures, the CLI driver, and the Postgres gate (see `fixtures`,
//!   `cli`, and `support_pg`)
//! - The production export, erase, and restore primitives (see `clio-compliance`)
//!
//! ## Boundary
//!
//! Changes to which cross-path claims are proven should normally require changes
//! here. Changes to a compliance primitive should not.
//!
//! Keep this target focused on proving the paths agree, not on the primitives it
//! drives.

// The scenario modules are filled in one at a time. Until a scenario runs, the
// helper it will need is defined but unused. This target is one binary, so an
// unused-in-one-scenario helper is not dead.
#![allow(dead_code)]

#[path = "../common/mod.rs"]
mod common;

mod cli;
mod fixtures;
mod support;
mod support_pg;

mod scenario_all_bank_peer;
mod scenario_archive_presence;
mod scenario_export_erase_restore;
mod scenario_intent_outcome_faults;
mod scenario_refusals_incomplete;
mod scenario_review_export;
