// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for resolving the sync device id one installation
//! may name explicitly.
//!
//! ## Owns
//! - The `CLIO_SYNC_DEVICE_ID` override, read as a trimmed, non-empty,
//!   NUL-free value
//!
//! ## Does not own
//! - Minting or storing the device id (see `clio-store`'s `OpsStore`)
//! - Database-target resolution (see `db_path`)
//!
//! ## Boundary
//!
//! Changes to how a caller overrides the sync device id should normally require
//! changes here. Changes to where the id is stored, or to how it is minted,
//! should not.
//!
//! Keep this module focused on the override.

use crate::db_path::env_value;

/// Environment variable naming an explicit sync device id.
pub const SYNC_DEVICE_ID_ENV: &str = "CLIO_SYNC_DEVICE_ID";

/// The sync device id the environment names, if any.
///
/// `None` means the installation's own stored id applies. The id belongs to the
/// store rather than to a database path, so an in-memory target has no special
/// case here. An empty, whitespace-only, or NUL-bearing value is treated as
/// absent. The lookup is injected so tests never read the host environment.
#[must_use]
pub fn resolve_sync_device_id<F>(get: &F) -> Option<String>
where
    F: Fn(&str) -> Option<String>,
{
    env_value(get, SYNC_DEVICE_ID_ENV)
}

#[cfg(test)]
#[path = "sync_device_id_tests.rs"]
mod tests;
