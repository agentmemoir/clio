// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is the entry point of the two-process peer-erasure acceptance
//! target: it mounts the shared spawned-binary support, the coordinator
//! fixture, the peer nodes, the injected faults, and the scenario bodies
//! registered for each backend.
//!
//! ## Owns
//! - The module list of the acceptance target
//!
//! ## Does not own
//! - Any scenario body (`scenarios_peer`, `scenarios_disposition`) or
//!   registration (`sqlite`, `postgres`)
//!
//! ## Boundary
//!
//! Adding or removing a scenario module normally requires a change here.
//! Changes to what a scenario asserts should not.
//!
//! Keep this target focused on the two-process peer erasure acceptance run.

#[path = "../sync_wire_support/mod.rs"]
mod sync_wire_support;

mod connect;
mod faults;
mod nodes;
mod postgres;
mod run;
mod scenarios_disposition;
mod scenarios_peer;
mod scenarios_replay;
mod sqlite;
mod support;
