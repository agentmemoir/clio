// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This integration target is responsible for measuring the durable erasure
//! authority: the denial recorded before key loss, its survival across a
//! restart and a handoff, and the failure-closed refusal API, on SQLite and
//! Postgres.
//!
//! ## Owns
//! - The module tree of the durable erasure authority measurement
//! - The per-scenario evidence the scenarios print
//!
//! ## Does not own
//! - The journal, its checks, and the denial step (see `clio-compliance`)
//! - The shared fixtures and probes (see `support`)
//!
//! ## Boundary
//!
//! Changes to which scenarios are measured should normally require changes
//! here. Changes to the journal or the store backends should not.
//!
//! Keep this target focused on measuring the durable authority, not on the
//! primitives it measures.

mod handoff;
mod membership;
mod postgres;
mod restore_guard;
mod restore_guard_crash;
mod restore_guard_fixtures;
mod restore_guard_recovery;
mod sqlite;
mod support;
