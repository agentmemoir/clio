// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This integration target is responsible for measuring what a restore does
//! with a denied subject on SQLite and Postgres: a pre-erasure archive
//! re-exposes the denied subject through each supported route, while a
//! guarded restore refuses one cut before a real or a synthetic denial.
//!
//! ## Owns
//! - The module tree of the old-archive restore measurement: its
//!   re-exposure, refusal, erase-call and member scenarios
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
