// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This integration target is responsible for measuring whether a guarded
//! restore refuses an archive that carries a denied subject: an archive cut
//! before a real compliance erasure, and archives the erasure journal denies
//! synthetically, on SQLite and Postgres.
//!
//! ## Owns
//! - The module tree of the old-archive restore measurement
//! - The per-route observation log the scenarios print
//!
//! ## Does not own
//! - The restore primitives (see `clio-store` and `clio-backup`)
//! - The shared fixtures and probes (see `support`)
//!
//! ## Boundary
//!
//! Changes to which scenarios are measured should normally require changes
//! here. Changes to a restore primitive should not.
//!
//! Keep this target focused on measuring restore readability, not on the
//! primitives it measures.

mod guard_cli;
mod guard_cli_erase_route;
mod guard_cli_erasure;
mod guard_cli_pg;
mod guard_cli_placement;
mod guard_fixtures;
mod members;
mod negative;
mod postgres;
mod sqlite;
mod support;
mod support_pg;

#[path = "../common/mod.rs"]
mod common;
